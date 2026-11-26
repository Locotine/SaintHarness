use crate::store::writer::{CommitHook, SqliteWriterHandle};
use saint_harness_schema::{CommitReceipt, SessionEvent};
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct BroadcastEnvelope {
    pub stream_id: i64,
    pub receipt: Option<CommitReceipt>,
    pub event: SessionEvent,
}

#[derive(Clone)]
pub struct EventBus {
    writer: SqliteWriterHandle,
    tx: broadcast::Sender<BroadcastEnvelope>,
    stream_seq: std::sync::Arc<AtomicI64>,
}

impl EventBus {
    pub fn new(writer: SqliteWriterHandle, capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(256));
        Self {
            writer,
            tx,
            stream_seq: std::sync::Arc::new(AtomicI64::new(0)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEnvelope> {
        self.tx.subscribe()
    }

    /// Commits durable event + projector + optional hook in 1 SQLite WAL transaction,
    /// then broadcasts SSE strictly AFTER commit succeeds (Law #5).
    pub async fn publish_durable(
        &self,
        event: SessionEvent,
        hook: Option<CommitHook>,
    ) -> Result<CommitReceipt, String> {
        let receipt = self.writer.publish_durable(event.clone(), hook).await?;
        let stream_id = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.tx.send(BroadcastEnvelope {
            stream_id,
            receipt: Some(receipt.clone()),
            event,
        });
        Ok(receipt)
    }

    /// Fans out ephemeral deltas (`TextDelta`, `ReasoningDelta`, `ToolProgress`) directly without touching SQLite.
    pub fn publish_ephemeral(&self, event: SessionEvent) {
        let stream_id = self.stream_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.tx.send(BroadcastEnvelope {
            stream_id,
            receipt: None,
            event,
        });
    }

    pub fn writer(&self) -> &SqliteWriterHandle {
        &self.writer
    }
}
