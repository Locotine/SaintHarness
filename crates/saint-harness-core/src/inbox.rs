use crate::store::EventStore;
use chrono::Utc;
use dashmap::DashMap;
use rusqlite::params;
use saint_harness_schema::{ContentPart, Message, Role};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use ulid::Ulid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxItemType {
    Prompt,
    Synthetic,
    Compaction,
    Move,
}

impl InboxItemType {
    pub fn as_str(&self) -> &'static str {
        match self {
            InboxItemType::Prompt => "prompt",
            InboxItemType::Synthetic => "synthetic",
            InboxItemType::Compaction => "compaction",
            InboxItemType::Move => "move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Steer,
    Queue,
}

impl DeliveryMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryMode::Steer => "steer",
            DeliveryMode::Queue => "queue",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub session_id: String,
    pub item_type: InboxItemType,
    pub delivery: DeliveryMode,
    pub enqueued_seq: i64,
    pub text: String,
    pub model_override: Option<String>,
}

#[derive(Clone)]
pub struct SessionInbox {
    store: EventStore,
    session_locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
}

impl SessionInbox {
    pub fn new(store: EventStore) -> Self {
        Self {
            store,
            session_locks: Arc::new(DashMap::new()),
        }
    }

    fn lock_for(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.session_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Idempotent enqueue keyed by `client_msg_id` (or generated ULID when omitted).
    pub async fn enqueue(
        &self,
        session_id: &str,
        client_msg_id: Option<String>,
        item_type: InboxItemType,
        delivery: DeliveryMode,
        text: String,
        model_override: Option<String>,
    ) -> Result<String, String> {
        let id = client_msg_id
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Ulid::new().to_string());
        let sid = session_id.to_string();
        let payload = serde_json::json!({
            "text": text,
            "model_override": model_override,
        })
        .to_string();
        let id_clone = id.clone();

        self.store
            .bus
            .writer()
            .exec_tx(move |tx| {
                let next_seq: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(enqueued_seq), 0) + 1 FROM session_inbox WHERE session_id = ?1",
                        params![sid],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                tx.execute(
                    "INSERT OR IGNORE INTO session_inbox
                     (id, session_id, item_type, delivery, enqueued_seq, payload_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        id_clone,
                        sid,
                        item_type.as_str(),
                        delivery.as_str(),
                        next_seq,
                        payload,
                        Utc::now().timestamp_millis()
                    ],
                )
                .map_err(|e| e.to_string())?;
                Ok(serde_json::Value::Null)
            })
            .await?;

        Ok(id)
    }

    /// Promotes queued inbox items atomically inside 1 SQLite tx under keyed session mutex:
    /// order = steer compaction -> steer prompts/synthetic -> at most 1 queue item (only when `allow_queue`).
    /// Never promotes when `has_unsettled_tool_calls` is true so `ToolCall`/`ToolResult` pairs stay unbroken.
    pub async fn promote(
        &self,
        session_id: &str,
        allow_queue: bool,
        has_unsettled_tool_calls: bool,
    ) -> Result<Vec<InboxItem>, String> {
        if has_unsettled_tool_calls {
            return Ok(Vec::new());
        }
        let lock = self.lock_for(session_id);
        let _guard = lock.lock().await;
        let sid = session_id.to_string();

        let val = self
            .store
            .bus
            .writer()
            .exec_tx(move |tx| {
                let mut stmt = tx
                    .prepare(
                        "SELECT id, session_id, item_type, delivery, enqueued_seq, payload_json
                         FROM session_inbox WHERE session_id = ?1 ORDER BY enqueued_seq ASC",
                    )
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map(params![sid], |r| {
                        let itype: String = r.get(2)?;
                        let deliv: String = r.get(3)?;
                        let payload_str: String = r.get(5)?;
                        let payload: serde_json::Value =
                            serde_json::from_str(&payload_str).unwrap_or_default();
                        Ok(InboxItem {
                            id: r.get(0)?,
                            session_id: r.get(1)?,
                            item_type: match itype.as_str() {
                                "compaction" => InboxItemType::Compaction,
                                "synthetic" => InboxItemType::Synthetic,
                                "move" => InboxItemType::Move,
                                _ => InboxItemType::Prompt,
                            },
                            delivery: if deliv == "steer" {
                                DeliveryMode::Steer
                            } else {
                                DeliveryMode::Queue
                            },
                            enqueued_seq: r.get(4)?,
                            text: payload
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            model_override: payload
                                .get("model_override")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string),
                        })
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                drop(stmt);

                let mut chosen: Vec<InboxItem> = Vec::new();
                // 1. Steer compaction first
                for item in &rows {
                    if item.delivery == DeliveryMode::Steer
                        && item.item_type == InboxItemType::Compaction
                    {
                        chosen.push(item.clone());
                    }
                }
                // 2. Steer prompts & synthetic items next
                for item in &rows {
                    if item.delivery == DeliveryMode::Steer
                        && item.item_type != InboxItemType::Compaction
                    {
                        chosen.push(item.clone());
                    }
                }
                // 3. At most 1 queued item when idle / turn boundary
                if allow_queue
                    && chosen.is_empty()
                    && let Some(first_q) = rows.iter().find(|i| i.delivery == DeliveryMode::Queue)
                {
                    chosen.push(first_q.clone());
                }

                for item in &chosen {
                    tx.execute("DELETE FROM session_inbox WHERE id = ?1", params![item.id])
                        .map_err(|e| e.to_string())?;
                }

                serde_json::to_value(&chosen).map_err(|e| e.to_string())
            })
            .await?;

        serde_json::from_value(val).map_err(|e| e.to_string())
    }
}

pub fn inbox_item_to_message(item: &InboxItem, parent_msg_id: Option<String>) -> Message {
    let role = match item.item_type {
        InboxItemType::Synthetic => Role::Synthetic,
        InboxItemType::Compaction => Role::Compaction,
        _ => Role::User,
    };
    Message {
        id: format!("msg_{}", Ulid::new()),
        parent_msg_id,
        role,
        parts: vec![ContentPart::Text {
            text: item.text.clone(),
            provider_meta: None,
        }],
        model_provider: None,
        model_id: item.model_override.clone(),
    }
}
