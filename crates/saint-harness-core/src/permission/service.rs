use crate::permission::policy::{evaluate_permission, pattern_matches};
use crate::permission::saved::{load_saved_approvals, save_approval};
use crate::store::EventStore;
use dashmap::DashMap;
use saint_harness_schema::{
    PermissionEffect, PermissionPolicy, PermissionReply, PermissionRequest, SessionEvent,
    ToolFailure, UserInputAnswers, UserInputRequest,
};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::oneshot;
use ulid::Ulid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allowed,
    RejectedWithFeedback(ToolFailure),
    UserDeclined,
}

struct PendingPermission {
    request: PermissionRequest,
    tx: oneshot::Sender<PermissionReply>,
}

struct PendingUserInput {
    tx: oneshot::Sender<UserInputAnswers>,
}

#[derive(Clone)]
pub struct PermissionService {
    store: EventStore,
    pending_perms: Arc<DashMap<String, PendingPermission>>,
    pending_inputs: Arc<DashMap<String, PendingUserInput>>,
}

impl PermissionService {
    pub fn new(store: EventStore) -> Self {
        Self {
            store,
            pending_perms: Arc::new(DashMap::new()),
            pending_inputs: Arc::new(DashMap::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn check_or_request(
        &self,
        policy: &PermissionPolicy,
        project_id: &str,
        workspace_root: &Path,
        session_id: &str,
        message_id: &str,
        action: &str,
        resource: &str,
        title: String,
        suggested_pattern: Option<String>,
        sandbox_summary: Option<String>,
        diff_preview: Option<String>,
        server_name: Option<&str>,
    ) -> Result<ApprovalDecision, String> {
        let saved = load_saved_approvals(&self.store, project_id).unwrap_or_default();
        let effect = evaluate_permission(
            policy,
            workspace_root,
            action,
            resource,
            server_name,
            &saved,
        );
        match effect {
            PermissionEffect::Allow => Ok(ApprovalDecision::Allowed),
            PermissionEffect::Deny => Ok(ApprovalDecision::RejectedWithFeedback(
                ToolFailure::BlockedError {
                    reason: format!("action `{action}` on `{resource}` denied by policy"),
                },
            )),
            PermissionEffect::Ask => {
                let req_id = format!("perm_{}", Ulid::new());
                let req = PermissionRequest {
                    request_id: req_id.clone(),
                    session_id: session_id.to_string(),
                    message_id: message_id.to_string(),
                    action: action.to_string(),
                    resource: resource.to_string(),
                    title,
                    suggested_pattern,
                    sandbox_summary,
                    diff_preview,
                };
                let (tx, rx) = oneshot::channel();
                self.pending_perms.insert(
                    req_id.clone(),
                    PendingPermission {
                        request: req.clone(),
                        tx,
                    },
                );
                let _ = self
                    .store
                    .bus
                    .publish_durable(
                        SessionEvent::PermissionRequested {
                            session_id: session_id.to_string(),
                            request: req,
                        },
                        None,
                    )
                    .await;

                let reply = rx.await.unwrap_or(PermissionReply::Reject);
                match reply {
                    PermissionReply::Once => Ok(ApprovalDecision::Allowed),
                    PermissionReply::Always { pattern } => {
                        save_approval(&self.store, project_id, action, resource, &pattern).await?;
                        // Batch auto-resolve other pending approvals matching this pattern.
                        self.auto_resolve_matching(action, &pattern);
                        Ok(ApprovalDecision::Allowed)
                    }
                    PermissionReply::Reject => {
                        // Bare reject cancels sibling pending approvals in the same message and emits UserDeclined.
                        self.reject_same_message(message_id);
                        Ok(ApprovalDecision::UserDeclined)
                    }
                    PermissionReply::RejectWithFeedback { feedback } => {
                        Ok(ApprovalDecision::RejectedWithFeedback(
                            ToolFailure::CorrectedError { feedback },
                        ))
                    }
                }
            }
        }
    }

    pub async fn reply_permission(
        &self,
        request_id: &str,
        reply: PermissionReply,
    ) -> Result<(), String> {
        let Some((_, pending)) = self.pending_perms.remove(request_id) else {
            return Err(format!("unknown permission request_id `{request_id}`"));
        };
        let session_id = pending.request.session_id.clone();
        let outcome = format!("{reply:?}");
        let _ = pending.tx.send(reply);
        let _ = self
            .store
            .bus
            .publish_durable(
                SessionEvent::PermissionResolved {
                    session_id,
                    request_id: request_id.to_string(),
                    outcome,
                },
                None,
            )
            .await;
        Ok(())
    }

    pub async fn request_user_input(
        &self,
        request: UserInputRequest,
    ) -> Result<UserInputAnswers, String> {
        let req_id = request.request_id.clone();
        let session_id = request.session_id.clone();
        let (tx, rx) = oneshot::channel();
        self.pending_inputs
            .insert(req_id.clone(), PendingUserInput { tx });
        self.store
            .bus
            .publish_durable(
                SessionEvent::UserInputRequested {
                    session_id: session_id.clone(),
                    request,
                },
                None,
            )
            .await?;

        let answers = rx
            .await
            .map_err(|_| "user input request cancelled".to_string())?;
        let _ = self
            .store
            .bus
            .publish_durable(
                SessionEvent::UserInputResolved {
                    session_id,
                    request_id: req_id,
                },
                None,
            )
            .await;
        Ok(answers)
    }

    pub fn reply_user_input(
        &self,
        request_id: &str,
        answers: UserInputAnswers,
    ) -> Result<(), String> {
        let Some((_, pending)) = self.pending_inputs.remove(request_id) else {
            return Err(format!("unknown user_input request_id `{request_id}`"));
        };
        pending
            .tx
            .send(answers)
            .map_err(|_| "user input waiter dropped".to_string())
    }

    fn auto_resolve_matching(&self, action: &str, pattern: &str) {
        let ignore_case = cfg!(any(target_os = "macos", target_os = "windows"));
        let keys: Vec<String> = self
            .pending_perms
            .iter()
            .filter(|e| {
                e.value().request.action == action
                    && pattern_matches(pattern, &e.value().request.resource, ignore_case)
            })
            .map(|e| e.key().clone())
            .collect();
        for k in keys {
            if let Some((_, p)) = self.pending_perms.remove(&k) {
                let _ = p.tx.send(PermissionReply::Once);
            }
        }
    }

    fn reject_same_message(&self, message_id: &str) {
        let keys: Vec<String> = self
            .pending_perms
            .iter()
            .filter(|e| e.value().request.message_id == message_id)
            .map(|e| e.key().clone())
            .collect();
        for k in keys {
            if let Some((_, p)) = self.pending_perms.remove(&k) {
                let _ = p.tx.send(PermissionReply::Reject);
            }
        }
    }
}
