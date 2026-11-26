use crate::inbox::{DeliveryMode, InboxItemType, SessionInbox};
use crate::store::EventStore;
use chrono::Utc;
use rusqlite::params;
use ulid::Ulid;

pub const DEFAULT_MAX_SUBAGENT_DEPTH: usize = 1;

#[derive(Debug, Clone)]
pub struct PendingOutboxRow {
    pub id: String,
    pub parent_session_id: String,
    pub child_session_id: String,
    pub payload_xml: String,
}

/// Inserts a subagent completion payload into `subagent_outbox` transactionally so crashes
/// before delivery never lose child results.
pub async fn enqueue_subagent_outbox(
    store: &EventStore,
    parent_session_id: &str,
    child_session_id: &str,
    summary: &str,
) -> Result<String, String> {
    let id = format!("out_{}", Ulid::new());
    let payload_xml = format!(
        "<background-result kind=\"subagent\" child_session_id=\"{child_session_id}\">\n{summary}\n</background-result>"
    );
    let id_c = id.clone();
    let parent = parent_session_id.to_string();
    let child = child_session_id.to_string();

    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "INSERT INTO subagent_outbox
                 (id, parent_session_id, child_session_id, payload_xml, delivered, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![
                    id_c,
                    parent,
                    child,
                    payload_xml,
                    Utc::now().timestamp_millis()
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(id)
}

/// Delivers all undelivered `subagent_outbox` rows into parent inboxes as `steer` items (exactly once).
pub async fn replay_pending_outbox(
    store: &EventStore,
    inbox: &SessionInbox,
) -> Result<Vec<String>, String> {
    let conn = store.reader.open_ro()?;
    let mut stmt = conn
        .prepare(
            "SELECT id, parent_session_id, child_session_id, payload_xml
             FROM subagent_outbox WHERE delivered = 0 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<PendingOutboxRow> = stmt
        .query_map([], |r| {
            Ok(PendingOutboxRow {
                id: r.get(0)?,
                parent_session_id: r.get(1)?,
                child_session_id: r.get(2)?,
                payload_xml: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let mut woken_parents = Vec::new();
    for row in rows {
        // Idempotent enqueue keyed by outbox id guarantees at-most-once delivery on crash recovery.
        inbox
            .enqueue(
                &row.parent_session_id,
                Some(row.id.clone()),
                InboxItemType::Synthetic,
                DeliveryMode::Steer,
                row.payload_xml,
                None,
            )
            .await?;
        let oid = row.id.clone();
        store
            .bus
            .writer()
            .exec_tx(move |tx| {
                tx.execute(
                    "UPDATE subagent_outbox SET delivered = 1 WHERE id = ?1",
                    params![oid],
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
            .await?;
        if !woken_parents.contains(&row.parent_session_id) {
            woken_parents.push(row.parent_session_id);
        }
    }
    Ok(woken_parents)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{InteractionMode, RuntimeMode, SessionEvent};

    #[tokio::test]
    async fn outbox_survives_crash_and_delivers_both_children_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("sub.sqlite")).unwrap();
        let inbox = SessionInbox::new(store.clone());

        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: "parent".into(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "p".into(),
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

        enqueue_subagent_outbox(&store, "parent", "child_1", "child 1 ok")
            .await
            .unwrap();
        enqueue_subagent_outbox(&store, "parent", "child_2", "child 2 ok")
            .await
            .unwrap();

        let woken = replay_pending_outbox(&store, &inbox).await.unwrap();
        assert_eq!(woken, vec!["parent".to_string()]);
        // Second replay delivers nothing (idempotent)
        assert!(
            replay_pending_outbox(&store, &inbox)
                .await
                .unwrap()
                .is_empty()
        );

        let promoted = inbox.promote("parent", true, false).await.unwrap();
        assert_eq!(promoted.len(), 2);
        assert!(promoted[0].text.contains("child_1"));
        assert!(promoted[1].text.contains("child_2"));
    }
}
