use chrono::Utc;
use rusqlite::{Transaction, params};
use saint_harness_schema::{Role, SessionEvent};

/// Deterministic projector applying durable `SessionEvent`s inside the single SQLite writer
/// transaction so event storage, relational projections, and FTS5 stay 100% consistent.
pub fn apply(tx: &Transaction<'_>, event: &SessionEvent, seq: i64) -> Result<(), rusqlite::Error> {
    let now = Utc::now().timestamp_millis();
    match event {
        SessionEvent::SessionCreated {
            session_id,
            parent_id,
            fork_source_id,
            title,
            agent_id,
            model_profile,
            cwd,
            workspace_root,
            ..
        } => {
            tx.execute(
                "INSERT OR IGNORE INTO session
                 (id, parent_id, fork_source_id, title, title_is_default, agent_id, model_profile,
                  cwd, workspace_root, active_branch, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, NULL, ?9, ?9)",
                params![
                    session_id,
                    parent_id,
                    fork_source_id,
                    title,
                    agent_id,
                    model_profile,
                    cwd,
                    workspace_root,
                    now
                ],
            )?;
        }
        SessionEvent::SessionTitleUpdated {
            session_id,
            title,
            is_default,
        } => {
            tx.execute(
                "UPDATE session SET title = ?1, title_is_default = ?2, updated_at = ?3 WHERE id = ?4",
                params![title, i32::from(*is_default), now, session_id],
            )?;
        }
        SessionEvent::SessionProfileSwitched {
            session_id,
            model_profile,
            ..
        } => {
            tx.execute(
                "UPDATE session SET model_profile = ?1, updated_at = ?2 WHERE id = ?3",
                params![model_profile, now, session_id],
            )?;
        }
        SessionEvent::SessionActiveBranchChanged {
            session_id,
            active_branch,
        } => {
            tx.execute(
                "UPDATE session SET active_branch = ?1, updated_at = ?2 WHERE id = ?3",
                params![active_branch, now, session_id],
            )?;
        }
        SessionEvent::MessageAppended {
            session_id,
            message,
            finish_reason,
            usage,
            error_json,
            snapshot_start,
            changed_files,
        } => {
            let msg_type = match message.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Synthetic => "synthetic",
                Role::System => "system",
                Role::Compaction => "compaction",
                Role::Tool => "shell",
            };
            let finish_str = finish_reason
                .map(|r| serde_json::to_string(&r).unwrap_or_default())
                .map(|s| s.trim_matches('"').to_string());
            let usage_json = usage.as_ref().and_then(|u| serde_json::to_string(u).ok());
            let changed_json = if changed_files.is_empty() {
                None
            } else {
                serde_json::to_string(changed_files).ok()
            };
            let data_json = serde_json::to_string(message).unwrap_or_else(|_| "{}".to_string());

            tx.execute(
                "INSERT OR REPLACE INTO session_message
                 (id, session_id, parent_msg_id, seq, msg_type, model_provider, model_id,
                  finish_reason, error_json, usage_json, snapshot_start, changed_files, data_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    message.id,
                    session_id,
                    message.parent_msg_id,
                    seq,
                    msg_type,
                    message.model_provider,
                    message.model_id,
                    finish_str,
                    error_json,
                    usage_json,
                    snapshot_start,
                    changed_json,
                    data_json,
                    now
                ],
            )?;
            tx.execute(
                "UPDATE session SET active_branch = ?1, updated_at = ?2 WHERE id = ?3",
                params![message.id, now, session_id],
            )?;
            let plain = message.plain_text();
            if !plain.is_empty() {
                tx.execute(
                    "INSERT INTO session_fts (session_id, message_id, content) VALUES (?1, ?2, ?3)",
                    params![session_id, message.id, plain],
                )?;
            }
        }
        SessionEvent::StepEnded {
            session_id,
            cost_usd,
            ..
        } => {
            tx.execute(
                "UPDATE session SET total_cost_usd = total_cost_usd + ?1, updated_at = ?2 WHERE id = ?3",
                params![cost_usd, now, session_id],
            )?;
        }
        SessionEvent::FileReadRecorded {
            session_id,
            canonical_path,
            blake3_hex,
            line_count,
            snapshot_zst,
            read_epoch,
        } => {
            tx.execute(
                "INSERT OR REPLACE INTO session_file_read_state
                 (session_id, canonical_path, last_blake3, last_line_count, snapshot_zst, read_epoch, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    session_id,
                    canonical_path,
                    blake3_hex,
                    *line_count as i64,
                    snapshot_zst,
                    *read_epoch as i64,
                    now
                ],
            )?;
        }
        SessionEvent::BackgroundJobUpdated {
            session_id,
            job_id,
            kind,
            command_preview,
            status,
            exit_code,
            spill_path,
        } => {
            let ended_at = if status == "running" { None } else { Some(now) };
            tx.execute(
                "INSERT INTO background_job
                 (id, session_id, kind, command_preview, status, exit_code, spill_path, started_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                   status = excluded.status,
                   exit_code = excluded.exit_code,
                   spill_path = COALESCE(excluded.spill_path, background_job.spill_path),
                   ended_at = COALESCE(excluded.ended_at, background_job.ended_at)",
                params![
                    job_id,
                    session_id,
                    kind,
                    command_preview,
                    status,
                    exit_code,
                    spill_path,
                    now,
                    ended_at
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}
