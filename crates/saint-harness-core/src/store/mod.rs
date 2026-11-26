pub mod bus;
pub mod projector;
pub mod queries;
pub mod writer;

pub use bus::*;
pub use queries::*;
pub use writer::*;

use rusqlite::{Connection, params};
use saint_harness_schema::SessionEvent;
use std::path::PathBuf;

#[derive(Clone)]
pub struct EventStore {
    pub db_path: PathBuf,
    pub bus: EventBus,
    pub reader: StoreReader,
}

impl EventStore {
    pub fn open(db_path: PathBuf) -> Result<Self, String> {
        let writer = SqliteWriterHandle::spawn(db_path.clone())?;
        let bus = EventBus::new(writer, 1024);
        let reader = StoreReader::new(db_path.clone());
        Ok(Self {
            db_path,
            bus,
            reader,
        })
    }

    /// Replays all durable events from the `event` journal into a fresh SQLite DB to verify deterministic projection.
    pub fn replay_into(&self, target_db: PathBuf) -> Result<usize, String> {
        let src = self.reader.open_ro()?;
        let mut stmt = src
            .prepare("SELECT seq, payload_json FROM event ORDER BY rowid ASC")
            .map_err(|e| e.to_string())?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        let mut dst = Connection::open(&target_db).map_err(|e| e.to_string())?;
        writer::configure_connection(&dst)?;
        dst.execute_batch(writer::INIT_SQL)
            .map_err(|e| e.to_string())?;
        let tx = dst
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        for (seq, json) in &rows {
            let ev: SessionEvent = serde_json::from_str(json).map_err(|e| e.to_string())?;
            projector::apply(&tx, &ev, *seq).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(rows.len())
    }

    pub async fn record_step_metric(&self, row: StepMetricRow) -> Result<(), String> {
        self.bus
            .writer()
            .exec_tx(move |tx| {
                tx.execute(
                    "INSERT INTO step_metrics
                     (id, session_id, step_num, model_profile, edit_strategy,
                      non_cached_in, cache_read_in, cache_write_in, output_tokens,
                      reasoning_tokens, cost_usd, ttft_ms, wall_ms, prefix_preserved,
                      edit_attempts, edit_first_try_ok, lsp_errors_caught,
                      raw_tool_bytes, context_tool_bytes, created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
                    params![
                        row.id,
                        row.session_id,
                        row.step_num,
                        row.model_profile,
                        row.edit_strategy,
                        row.non_cached_in,
                        row.cache_read_in,
                        row.cache_write_in,
                        row.output_tokens,
                        row.reasoning_tokens,
                        row.cost_usd,
                        row.ttft_ms,
                        row.wall_ms,
                        i64::from(row.prefix_preserved),
                        row.edit_attempts,
                        row.edit_first_try_ok,
                        row.lsp_errors_caught,
                        row.raw_tool_bytes,
                        row.context_tool_bytes,
                        chrono::Utc::now().timestamp_millis()
                    ],
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{ContentPart, InteractionMode, Message, Role, RuntimeMode};

    #[tokio::test]
    async fn thirty_two_concurrent_writers_produce_1000_contiguous_seqs_zero_busy_and_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("store.sqlite");
        let store = EventStore::open(db).unwrap();

        let sid = "sess_stress".to_string();
        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: sid.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "stress".into(),
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

        // 32 parallel tasks writing 999 more events (total 1000 contiguous seqs 1..=1000)
        let mut set = tokio::task::JoinSet::new();
        for worker in 0..32usize {
            let bus = store.bus.clone();
            let sid = sid.clone();
            let count = if worker < 7 { 32 } else { 31 }; // 7*32 + 25*31 = 224 + 775 = 999
            set.spawn(async move {
                for j in 0..count {
                    let msg = Message {
                        id: format!("m_{worker}_{j}"),
                        parent_msg_id: None,
                        role: Role::User,
                        parts: vec![ContentPart::Text {
                            text: format!("wal_stress_token_{worker}_{j}"),
                            provider_meta: None,
                        }],
                        model_provider: None,
                        model_id: None,
                    };
                    bus.publish_durable(
                        SessionEvent::MessageAppended {
                            session_id: sid.clone(),
                            message: msg,
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
                }
            });
        }
        while let Some(res) = set.join_next().await {
            res.unwrap();
        }

        let seqs = store.reader.list_event_sequences(&sid).unwrap();
        assert_eq!(seqs.len(), 1000);
        assert_eq!(seqs, (1..=1000).collect::<Vec<i64>>());

        let fts = store
            .reader
            .search_fts("wal_stress_token_15_10", 5)
            .unwrap();
        assert_eq!(fts.len(), 1);

        let replay_db = tmp.path().join("replay.sqlite");
        let replayed = store.replay_into(replay_db.clone()).unwrap();
        assert_eq!(replayed, 1000);
        let replay_reader = StoreReader::new(replay_db);
        assert_eq!(replay_reader.list_all_messages(&sid).unwrap().len(), 999);
    }
}
