pub mod policy;
pub mod saved;
pub mod service;

pub use policy::*;
pub use saved::*;
pub use service::*;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::EventStore;
    use saint_harness_schema::{
        InteractionMode, PermissionEffect, PermissionPolicy, PermissionReply, PermissionRule,
        RuntimeMode,
    };
    use std::path::PathBuf;

    #[test]
    fn lattice_rules_modes_and_dangerous_pattern_blocks() {
        let ws = PathBuf::from("/repo");
        let mut policy = PermissionPolicy {
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: InteractionMode::Build,
            ..PermissionPolicy::default()
        };
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", "/repo/src/a.rs", None, &[]),
            PermissionEffect::Ask
        );
        policy.runtime_mode = RuntimeMode::Auto;
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", "/repo/src/a.rs", None, &[]),
            PermissionEffect::Allow
        );
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", "/etc/hosts", None, &[]),
            PermissionEffect::Ask
        );
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", "/repo/../etc/hosts", None, &[]),
            PermissionEffect::Ask
        );
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", r"repo\.env.local", None, &[]),
            PermissionEffect::Ask
        );
        policy.runtime_mode = RuntimeMode::FullAccess;
        assert_eq!(
            evaluate_permission(&policy, &ws, "shell", "rm -rf /tmp/x", None, &[]),
            PermissionEffect::Allow
        );
        // Restrict layer overrides even FullAccess with Deny
        policy.restrict_rules.push(PermissionRule {
            action: "shell".into(),
            resource_pattern: "*rm -rf*".into(),
            effect: PermissionEffect::Deny,
        });
        assert_eq!(
            evaluate_permission(&policy, &ws, "shell", "rm -rf /tmp/x", None, &[]),
            PermissionEffect::Deny
        );
        // Plan mode denies edits
        policy.interaction_mode = InteractionMode::Plan;
        assert_eq!(
            evaluate_permission(&policy, &ws, "edit", "/repo/src/a.rs", None, &[]),
            PermissionEffect::Deny
        );
        // Dangerous saved patterns rejected
        assert!(validate_saveable_pattern("read", ".env", "*").is_err());
        assert!(validate_saveable_pattern("shell", "sudo ls", "sudo *").is_err());
        assert!(validate_saveable_pattern("shell", "bash -c id", "bash -c *").is_err());
        assert!(validate_saveable_pattern("shell", "cargo test", "cargo test *").is_ok());
    }

    #[tokio::test]
    async fn bare_reject_returns_user_declined_and_always_auto_resolves_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("perm.sqlite")).unwrap();
        let svc = PermissionService::new(store.clone());
        let policy = PermissionPolicy::default();

        let mut rx = store.bus.subscribe();
        let svc2 = svc.clone();
        let policy2 = policy.clone();
        let ws = tmp.path().to_path_buf();
        let h = tokio::spawn(async move {
            svc2.check_or_request(
                &policy2,
                "proj",
                &ws,
                "s1",
                "m1",
                "shell",
                "cargo test",
                "Approve?".into(),
                Some("cargo test *".into()),
                None,
                None,
                None,
            )
            .await
            .unwrap()
        });

        let req_id = loop {
            if let Ok(env) = rx.recv().await
                && let saint_harness_schema::SessionEvent::PermissionRequested { request, .. } =
                    env.event
            {
                break request.request_id;
            }
        };
        svc.reply_permission(&req_id, PermissionReply::Reject)
            .await
            .unwrap();
        assert_eq!(h.await.unwrap(), ApprovalDecision::UserDeclined);
    }
}
