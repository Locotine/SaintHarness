use crate::store::EventStore;
use chrono::Utc;
use rusqlite::params;
use saint_harness_schema::{ContentPart, Message, Role, SessionEvent, ToolResultBlock};
use std::collections::HashSet;
use ulid::Ulid;

pub const DEFAULT_MAX_RESUME_ATTEMPTS: i64 = 10;

#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub resumed_sessions: Vec<String>,
    pub blocked_crash_loop_sessions: Vec<String>,
    pub settled_orphan_tool_calls: usize,
    pub orphaned_background_jobs: usize,
}

/// Scans sessions with an active `claim_owner` left over after a crash:
/// - marks stale running `background_job` rows as `orphaned`
/// - settles orphan `ToolCall` parts lacking a matching `ToolResult` as `interrupted`
/// - enforces `DEFAULT_MAX_RESUME_ATTEMPTS = 10` crash-loop guard.
pub async fn run_boot_recovery(store: &EventStore) -> Result<RecoveryReport, String> {
    let mut report = RecoveryReport::default();

    // 1. Mark stale running background jobs as orphaned
    let orphaned_jobs = store
        .bus
        .writer()
        .exec_tx(|tx| {
            let n = tx
                .execute(
                    "UPDATE background_job SET status = 'orphaned', ended_at = ?1 WHERE status = 'running'",
                    params![Utc::now().timestamp_millis()],
                )
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!(n))
        })
        .await?;
    report.orphaned_background_jobs = orphaned_jobs.as_u64().unwrap_or(0) as usize;

    // 2. Scan claimed sessions
    let sessions = store.reader.list_sessions()?;
    for sess in sessions {
        if sess.claim_owner.is_none() {
            continue;
        }
        if sess.resume_attempts >= DEFAULT_MAX_RESUME_ATTEMPTS {
            let sid = sess.id.clone();
            store
                .bus
                .writer()
                .exec_tx(move |tx| {
                    tx.execute(
                        "UPDATE session SET claim_owner = NULL WHERE id = ?1",
                        params![sid],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(serde_json::Value::Null)
                })
                .await?;
            report.blocked_crash_loop_sessions.push(sess.id);
            continue;
        }

        let settled = settle_stale_tool_calls(store, &sess.id).await?;
        report.settled_orphan_tool_calls += settled;

        let sid = sess.id.clone();
        store
            .bus
            .writer()
            .exec_tx(move |tx| {
                tx.execute(
                    "UPDATE session SET claim_owner = NULL, resume_attempts = resume_attempts + 1 WHERE id = ?1",
                    params![sid],
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
            .await?;
        report.resumed_sessions.push(sess.id);
    }

    Ok(report)
}

pub async fn acquire_session_claim(
    store: &EventStore,
    session_id: &str,
    owner: &str,
) -> Result<(), String> {
    let sid = session_id.to_string();
    let owner = owner.to_string();
    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "UPDATE session SET claim_owner = ?1, updated_at = ?2 WHERE id = ?3",
                params![owner, Utc::now().timestamp_millis(), sid],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(())
}

pub async fn release_session_claim(store: &EventStore, session_id: &str) -> Result<(), String> {
    let sid = session_id.to_string();
    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "UPDATE session SET claim_owner = NULL, updated_at = ?1 WHERE id = ?2",
                params![Utc::now().timestamp_millis(), sid],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(())
}

pub async fn settle_stale_tool_calls(
    store: &EventStore,
    session_id: &str,
) -> Result<usize, String> {
    let chain = store.reader.walk_active_branch(session_id)?;
    let mut called: Vec<(String, String)> = Vec::new();
    let mut settled_ids: HashSet<String> = HashSet::new();

    for row in &chain {
        for part in &row.message.parts {
            match part {
                ContentPart::ToolCall {
                    id,
                    name,
                    provider_executed: false,
                    ..
                } => {
                    called.push((id.clone(), name.clone()));
                }
                ContentPart::ToolResult { call_id, .. } => {
                    settled_ids.insert(call_id.clone());
                }
                _ => {}
            }
        }
    }

    let missing: Vec<(String, String)> = called
        .into_iter()
        .filter(|(id, _)| !settled_ids.contains(id))
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }

    let parent_id = chain.last().map(|r| r.id.clone());
    let count = missing.len();
    let parts: Vec<ContentPart> = missing
        .into_iter()
        .map(|(call_id, name)| ContentPart::ToolResult {
            call_id,
            name,
            content: vec![ToolResultBlock::Text {
                text: "interrupted: process crashed before tool settled".to_string(),
            }],
            is_error: true,
        })
        .collect();

    let msg = Message {
        id: format!("msg_{}", Ulid::new()),
        parent_msg_id: parent_id,
        role: Role::Tool,
        parts,
        model_provider: None,
        model_id: None,
    };

    store
        .bus
        .publish_durable(
            SessionEvent::MessageAppended {
                session_id: session_id.to_string(),
                message: msg,
                finish_reason: None,
                usage: None,
                error_json: None,
                snapshot_start: None,
                changed_files: vec![],
            },
            None,
        )
        .await?;
    Ok(count)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{InteractionMode, RuntimeMode};

    #[tokio::test]
    async fn boot_recovery_settles_orphan_tools_and_blocks_after_10_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("rec.sqlite")).unwrap();
        let sid = "s_crash".to_string();

        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: sid.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "crash".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: "/tmp".into(),
                    workspace_root: "/tmp".into(),
                    runtime_mode: RuntimeMode::Auto,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();

        store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: sid.clone(),
                    message: Message {
                        id: "m1".into(),
                        parent_msg_id: None,
                        role: Role::Assistant,
                        parts: vec![ContentPart::ToolCall {
                            id: "orphan_1".into(),
                            name: "shell".into(),
                            arguments: serde_json::json!({"command": "sleep 10"}),
                            provider_executed: false,
                            signature: None,
                        }],
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec![],
                },
                None,
            )
            .await
            .unwrap();

        acquire_session_claim(&store, &sid, "pid-old")
            .await
            .unwrap();
        let r1 = run_boot_recovery(&store).await.unwrap();
        assert_eq!(r1.resumed_sessions, vec![sid.clone()]);
        assert_eq!(r1.settled_orphan_tool_calls, 1);

        // Simulate 10th attempt reached -> 11th boot must block crash loop
        let sid2 = sid.clone();
        store
            .bus
            .writer()
            .exec_tx(move |tx| {
                tx.execute(
                    "UPDATE session SET claim_owner = 'pid-loop', resume_attempts = 10 WHERE id = ?1",
                    params![sid2],
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
            .await
            .unwrap();

        let r2 = run_boot_recovery(&store).await.unwrap();
        assert_eq!(r2.blocked_crash_loop_sessions, vec![sid]);
    }
}
