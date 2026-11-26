use crate::store::writer::configure_connection;
use rusqlite::{Connection, OpenFlags, params};
use saint_harness_schema::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub parent_id: Option<String>,
    pub fork_source_id: Option<String>,
    pub title: String,
    pub title_is_default: bool,
    pub agent_id: String,
    pub model_profile: String,
    pub cwd: String,
    pub workspace_root: String,
    pub active_branch: Option<String>,
    pub claim_owner: Option<String>,
    pub resume_attempts: i64,
    pub revert_staged_seq: Option<i64>,
    pub revert_orig_snap: Option<String>,
    pub total_cost_usd: f64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredMessageRow {
    pub id: String,
    pub parent_msg_id: Option<String>,
    pub seq: i64,
    pub finish_reason: Option<String>,
    pub error_json: Option<String>,
    pub snapshot_start: Option<String>,
    pub message: Message,
}

#[derive(Debug, Clone)]
pub struct FileReadStateRow {
    pub canonical_path: String,
    pub last_blake3: String,
    pub last_line_count: usize,
    pub snapshot_zst: Option<Vec<u8>>,
    pub read_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundJobRow {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub command_preview: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub spill_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepMetricRow {
    pub id: String,
    pub session_id: String,
    pub step_num: i64,
    pub model_profile: String,
    pub edit_strategy: String,
    pub non_cached_in: i64,
    pub cache_read_in: i64,
    pub cache_write_in: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: f64,
    pub ttft_ms: i64,
    pub wall_ms: i64,
    pub prefix_preserved: bool,
    pub edit_attempts: i64,
    pub edit_first_try_ok: i64,
    pub lsp_errors_caught: i64,
    pub raw_tool_bytes: i64,
    pub context_tool_bytes: i64,
}

#[derive(Clone)]
pub struct StoreReader {
    db_path: PathBuf,
}

impl StoreReader {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn open_ro(&self) -> Result<Connection, String> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| e.to_string())?;
        let _ = configure_connection(&conn);
        Ok(conn)
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, parent_id, fork_source_id, title, title_is_default, agent_id,
                        model_profile, cwd, workspace_root, active_branch, claim_owner,
                        resume_attempts, revert_staged_seq, revert_orig_snap, total_cost_usd,
                        created_at, updated_at
                 FROM session ORDER BY updated_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], row_to_session)
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, parent_id, fork_source_id, title, title_is_default, agent_id,
                        model_profile, cwd, workspace_root, active_branch, claim_owner,
                        resume_attempts, revert_staged_seq, revert_orig_snap, total_cost_usd,
                        created_at, updated_at
                 FROM session WHERE id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![session_id], row_to_session)
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(Ok(rec)) => Ok(Some(rec)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    pub fn list_event_sequences(&self, aggregate_id: &str) -> Result<Vec<i64>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare("SELECT seq FROM event WHERE aggregate_id = ?1 ORDER BY seq ASC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![aggregate_id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn high_water_seq(&self) -> Result<i64, String> {
        let conn = self.open_ro()?;
        let v: Option<i64> = conn
            .query_row("SELECT MAX(seq) FROM event", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(v.unwrap_or(0))
    }

    pub fn list_all_messages(&self, session_id: &str) -> Result<Vec<StoredMessageRow>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, parent_msg_id, seq, finish_reason, error_json, snapshot_start, data_json
                 FROM session_message WHERE session_id = ?1 ORDER BY seq ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                let data_json: String = r.get(6)?;
                let message: Message = serde_json::from_str(&data_json).unwrap_or(Message {
                    id: r.get(0)?,
                    parent_msg_id: r.get(1)?,
                    role: saint_harness_schema::Role::User,
                    parts: vec![],
                    model_provider: None,
                    model_id: None,
                });
                Ok(StoredMessageRow {
                    id: r.get(0)?,
                    parent_msg_id: r.get(1)?,
                    seq: r.get(2)?,
                    finish_reason: r.get(3)?,
                    error_json: r.get(4)?,
                    snapshot_start: r.get(5)?,
                    message,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Walks `parent_msg_id` backwards from `session.active_branch` (filtering `seq <= revert_staged_seq`)
    /// so sibling `/tree` branches are never mixed (Trap #7).
    pub fn walk_active_branch(&self, session_id: &str) -> Result<Vec<StoredMessageRow>, String> {
        let Some(sess) = self.get_session(session_id)? else {
            return Ok(Vec::new());
        };
        let all = self.list_all_messages(session_id)?;
        let mut by_id: HashMap<String, StoredMessageRow> = all
            .into_iter()
            .filter(|m| sess.revert_staged_seq.is_none_or(|limit| m.seq <= limit))
            .map(|m| (m.id.clone(), m))
            .collect();

        let head = match sess.active_branch {
            Some(ref id) if by_id.contains_key(id) => Some(id.clone()),
            _ => by_id.values().max_by_key(|m| m.seq).map(|m| m.id.clone()),
        };

        let mut chain = Vec::new();
        let mut cursor = head;
        let mut visited = std::collections::HashSet::new();
        while let Some(id) = cursor {
            if !visited.insert(id.clone()) {
                break;
            }
            if let Some(row) = by_id.remove(&id) {
                cursor = row.parent_msg_id.clone();
                chain.push(row);
            } else {
                break;
            }
        }
        chain.reverse();
        Ok(chain)
    }

    pub fn search_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT session_id, message_id, content FROM session_fts
                 WHERE session_fts MATCH ?1 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![query, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn get_file_read_state(
        &self,
        session_id: &str,
        canonical_path: &str,
    ) -> Result<Option<FileReadStateRow>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT canonical_path, last_blake3, last_line_count, snapshot_zst, read_epoch
                 FROM session_file_read_state WHERE session_id = ?1 AND canonical_path = ?2",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query_map(params![session_id, canonical_path], |r| {
                Ok(FileReadStateRow {
                    canonical_path: r.get(0)?,
                    last_blake3: r.get(1)?,
                    last_line_count: r.get::<_, i64>(2)? as usize,
                    snapshot_zst: r.get(3)?,
                    read_epoch: r.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            Some(Err(e)) => Err(e.to_string()),
            None => Ok(None),
        }
    }

    pub fn list_background_jobs(&self, session_id: &str) -> Result<Vec<BackgroundJobRow>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, kind, command_preview, status, exit_code, spill_path
                 FROM background_job WHERE session_id = ?1 ORDER BY started_at ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(BackgroundJobRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    kind: r.get(2)?,
                    command_preview: r.get(3)?,
                    status: r.get(4)?,
                    exit_code: r.get(5)?,
                    spill_path: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn list_step_metrics(&self, session_id: &str) -> Result<Vec<StepMetricRow>, String> {
        let conn = self.open_ro()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, step_num, model_profile, edit_strategy,
                        non_cached_in, cache_read_in, cache_write_in, output_tokens,
                        reasoning_tokens, cost_usd, ttft_ms, wall_ms, prefix_preserved,
                        edit_attempts, edit_first_try_ok, lsp_errors_caught,
                        raw_tool_bytes, context_tool_bytes
                 FROM step_metrics WHERE session_id = ?1 ORDER BY step_num ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![session_id], |r| {
                Ok(StepMetricRow {
                    id: r.get(0)?,
                    session_id: r.get(1)?,
                    step_num: r.get(2)?,
                    model_profile: r.get(3)?,
                    edit_strategy: r.get(4)?,
                    non_cached_in: r.get(5)?,
                    cache_read_in: r.get(6)?,
                    cache_write_in: r.get(7)?,
                    output_tokens: r.get(8)?,
                    reasoning_tokens: r.get(9)?,
                    cost_usd: r.get(10)?,
                    ttft_ms: r.get(11)?,
                    wall_ms: r.get(12)?,
                    prefix_preserved: r.get::<_, i64>(13)? != 0,
                    edit_attempts: r.get(14)?,
                    edit_first_try_ok: r.get(15)?,
                    lsp_errors_caught: r.get(16)?,
                    raw_tool_bytes: r.get(17)?,
                    context_tool_bytes: r.get(18)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
}

fn row_to_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        fork_source_id: r.get(2)?,
        title: r.get(3)?,
        title_is_default: r.get::<_, i64>(4)? != 0,
        agent_id: r.get(5)?,
        model_profile: r.get(6)?,
        cwd: r.get(7)?,
        workspace_root: r.get(8)?,
        active_branch: r.get(9)?,
        claim_owner: r.get(10)?,
        resume_attempts: r.get(11)?,
        revert_staged_seq: r.get(12)?,
        revert_orig_snap: r.get(13)?,
        total_cost_usd: r.get(14)?,
        created_at: r.get(15)?,
        updated_at: r.get(16)?,
    })
}
