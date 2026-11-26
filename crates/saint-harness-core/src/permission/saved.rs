use crate::store::EventStore;
use chrono::Utc;
use rusqlite::params;
use saint_harness_schema::{PermissionEffect, PermissionRule};
use ulid::Ulid;

/// Validates that a user-saved `Always(pattern)` approval does not create a dangerous wildcard rule.
pub fn validate_saveable_pattern(
    action: &str,
    resource: &str,
    pattern: &str,
) -> Result<(), String> {
    let p = pattern.trim();
    if p.is_empty() {
        return Err("saved approval pattern cannot be empty".to_string());
    }
    if action == "read" && resource.to_ascii_lowercase().contains(".env") && p == "*" {
        return Err("refusing to save wildcard '*' approval for .env files".to_string());
    }
    let lower = p.to_ascii_lowercase();
    if action == "shell"
        && (lower == "*"
            || lower.starts_with("sudo ")
            || lower == "sudo"
            || lower.starts_with("bash -c")
            || lower.starts_with("sh -c")
            || lower.starts_with("rm -rf"))
    {
        return Err(format!(
            "refusing to save dangerous shell approval pattern `{p}`"
        ));
    }
    Ok(())
}

pub async fn save_approval(
    store: &EventStore,
    project_id: &str,
    action: &str,
    resource: &str,
    pattern: &str,
) -> Result<(), String> {
    validate_saveable_pattern(action, resource, pattern)?;
    let id = Ulid::new().to_string();
    let pid = project_id.to_string();
    let act = action.to_string();
    let pat = pattern.to_string();
    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "INSERT OR IGNORE INTO permission_saved (id, project_id, action, resource, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, pid, act, pat, Utc::now().timestamp_millis()],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(())
}

pub fn load_saved_approvals(
    store: &EventStore,
    project_id: &str,
) -> Result<Vec<PermissionRule>, String> {
    let conn = store.reader.open_ro()?;
    let mut stmt = conn
        .prepare("SELECT action, resource FROM permission_saved WHERE project_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(PermissionRule {
                action: r.get(0)?,
                resource_pattern: r.get(1)?,
                effect: PermissionEffect::Allow,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
