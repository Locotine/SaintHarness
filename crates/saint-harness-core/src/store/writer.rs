use crate::store::projector;
use chrono::Utc;
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use saint_harness_schema::{CommitReceipt, SessionEvent};
use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, sync_channel};
use tokio::sync::oneshot;
use ulid::Ulid;

pub const INIT_SQL: &str = include_str!("migrations/0001_init.sql");

pub type CommitHook =
    Box<dyn FnOnce(&Transaction<'_>, &CommitReceipt) -> Result<(), rusqlite::Error> + Send>;
pub type RawTxOp = Box<dyn FnOnce(&Transaction<'_>) -> Result<serde_json::Value, String> + Send>;

pub enum WriteCmd {
    PublishDurable {
        event: Box<SessionEvent>,
        hook: Option<CommitHook>,
        reply: oneshot::Sender<Result<CommitReceipt, String>>,
    },
    RawTx {
        op: RawTxOp,
        reply: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct SqliteWriterHandle {
    tx: SyncSender<WriteCmd>,
}

impl SqliteWriterHandle {
    pub fn spawn(db_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut conn = Connection::open(&db_path).map_err(|e| e.to_string())?;
        configure_connection(&conn)?;
        conn.execute_batch(INIT_SQL)
            .map_err(|e| format!("failed to run 0001_init.sql: {e}"))?;

        let (tx, rx) = sync_channel::<WriteCmd>(2048);
        std::thread::Builder::new()
            .name("sh-sqlite-writer".to_string())
            .spawn(move || {
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        WriteCmd::PublishDurable { event, hook, reply } => {
                            let res = apply_durable(&mut conn, &event, hook);
                            let _ = reply.send(res);
                        }
                        WriteCmd::RawTx { op, reply } => {
                            let res = run_raw_tx(&mut conn, op);
                            let _ = reply.send(res);
                        }
                        WriteCmd::Shutdown => break,
                    }
                }
            })
            .map_err(|e| format!("failed to spawn sh-sqlite-writer thread: {e}"))?;

        Ok(Self { tx })
    }

    pub async fn publish_durable(
        &self,
        event: SessionEvent,
        hook: Option<CommitHook>,
    ) -> Result<CommitReceipt, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WriteCmd::PublishDurable {
                event: Box::new(event),
                hook,
                reply: reply_tx,
            })
            .map_err(|_| "sh-sqlite-writer channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "sh-sqlite-writer dropped reply".to_string())?
    }

    pub async fn exec_tx<F>(&self, op: F) -> Result<serde_json::Value, String>
    where
        F: FnOnce(&Transaction<'_>) -> Result<serde_json::Value, String> + Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(WriteCmd::RawTx {
                op: Box::new(op),
                reply: reply_tx,
            })
            .map_err(|_| "sh-sqlite-writer channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "sh-sqlite-writer dropped reply".to_string())?
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(WriteCmd::Shutdown);
    }
}

pub fn configure_connection(conn: &Connection) -> Result<(), String> {
    conn.busy_timeout(std::time::Duration::from_millis(5_000))
        .map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA synchronous = NORMAL;",
    )
    .map_err(|e| e.to_string())
}

fn apply_durable(
    conn: &mut Connection,
    event: &SessionEvent,
    hook: Option<CommitHook>,
) -> Result<CommitReceipt, String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;

    let aggregate_id = event.session_id().to_string();
    tx.execute(
        "INSERT INTO event_sequence (aggregate_id, seq) VALUES (?1, 1)
         ON CONFLICT(aggregate_id) DO UPDATE SET seq = seq + 1",
        params![aggregate_id],
    )
    .map_err(|e| e.to_string())?;

    let seq: i64 = tx
        .query_row(
            "SELECT seq FROM event_sequence WHERE aggregate_id = ?1",
            params![aggregate_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;

    let event_id = Ulid::new().to_string();
    let created_at = Utc::now().timestamp_millis();
    let payload_json = serde_json::to_string(event).map_err(|e| e.to_string())?;

    tx.execute(
        "INSERT INTO event (id, aggregate_id, seq, event_type, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event_id,
            aggregate_id,
            seq,
            event.event_type_name(),
            payload_json,
            created_at
        ],
    )
    .map_err(|e| e.to_string())?;

    projector::apply(&tx, event, seq).map_err(|e| e.to_string())?;

    let receipt = CommitReceipt {
        event_id,
        aggregate_id,
        seq,
        created_at,
    };

    if let Some(hook_fn) = hook {
        hook_fn(&tx, &receipt).map_err(|e| e.to_string())?;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(receipt)
}

fn run_raw_tx(conn: &mut Connection, op: RawTxOp) -> Result<serde_json::Value, String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let val = op(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(val)
}
