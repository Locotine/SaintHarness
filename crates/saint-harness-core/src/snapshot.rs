use crate::store::EventStore;
use async_trait::async_trait;
use ignore::WalkBuilder;
use imara_diff::{Algorithm, Diff, InternedInput, UnifiedDiffConfig};
use rusqlite::params;
use saint_harness_schema::SnapshotEngine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use ulid::Ulid;

const MAX_UNTRACKED_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotManifest {
    id: String,
    files: BTreeMap<String, String>,
}

/// Shadow snapshot engine stored separately under `<data>/snapshot/<project_hash>/<session_id>`
/// without touching the user's `.git` repository, skipping files > 2 MiB.
#[derive(Debug, Clone)]
pub struct ShadowSnapshotEngine {
    snapshot_root: PathBuf,
    disabled: bool,
}

impl ShadowSnapshotEngine {
    pub fn new(snapshot_root: PathBuf, disable_internal_git_snapshot: bool) -> Self {
        Self {
            snapshot_root,
            disabled: disable_internal_git_snapshot,
        }
    }

    fn session_dir(&self, workspace_root: &Path, session_id: &str) -> PathBuf {
        let proj_hash = blake3::hash(workspace_root.to_string_lossy().as_bytes())
            .to_hex()
            .chars()
            .take(12)
            .collect::<String>();
        self.snapshot_root.join(proj_hash).join(session_id)
    }

    fn blobs_dir(&self, workspace_root: &Path, session_id: &str) -> PathBuf {
        self.session_dir(workspace_root, session_id).join("blobs")
    }

    fn manifest_path(&self, workspace_root: &Path, session_id: &str, snap_id: &str) -> PathBuf {
        self.session_dir(workspace_root, session_id)
            .join(format!("{snap_id}.json"))
    }
}

#[async_trait]
impl SnapshotEngine for ShadowSnapshotEngine {
    async fn capture(&self, workspace_root: &Path, session_id: &str) -> Result<String, String> {
        if self.disabled {
            return Ok("disabled".to_string());
        }
        let snap_id = format!(
            "snap_{}",
            Ulid::new().to_string()[..10].to_ascii_lowercase()
        );
        let blobs = self.blobs_dir(workspace_root, session_id);
        fs::create_dir_all(&blobs).map_err(|e| e.to_string())?;

        let mut files = BTreeMap::new();
        for entry in WalkBuilder::new(workspace_root)
            .hidden(false)
            .git_ignore(true)
            .filter_entry(|e| e.file_name() != ".git" && e.file_name() != "target")
            .build()
            .flatten()
        {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.len() > MAX_UNTRACKED_FILE_BYTES {
                continue;
            }
            let Ok(data) = fs::read(p) else {
                continue;
            };
            let hash = blake3::hash(&data).to_hex().to_string();
            let blob_path = blobs.join(&hash);
            if !blob_path.exists()
                && let Ok(compressed) = zstd::stream::encode_all(data.as_slice(), 3)
            {
                let _ = fs::write(&blob_path, compressed);
            }
            let rel = p
                .strip_prefix(workspace_root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(rel, hash);
        }

        let manifest = SnapshotManifest {
            id: snap_id.clone(),
            files,
        };
        let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
        fs::write(
            self.manifest_path(workspace_root, session_id, &snap_id),
            json,
        )
        .map_err(|e| e.to_string())?;
        Ok(snap_id)
    }

    async fn restore(
        &self,
        workspace_root: &Path,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<String>, String> {
        if self.disabled || snapshot_id == "disabled" {
            return Ok(Vec::new());
        }
        let manifest_raw =
            fs::read_to_string(self.manifest_path(workspace_root, session_id, snapshot_id))
                .map_err(|e| format!("snapshot `{snapshot_id}` not found: {e}"))?;
        let manifest: SnapshotManifest =
            serde_json::from_str(&manifest_raw).map_err(|e| e.to_string())?;
        let blobs = self.blobs_dir(workspace_root, session_id);
        let mut restored = Vec::new();

        for (rel, hash) in &manifest.files {
            let blob_raw = fs::read(blobs.join(hash)).map_err(|e| e.to_string())?;
            let decoded =
                zstd::stream::decode_all(blob_raw.as_slice()).map_err(|e| e.to_string())?;
            let dest = workspace_root.join(rel);
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&dest, decoded).map_err(|e| e.to_string())?;
            restored.push(rel.clone());
        }
        Ok(restored)
    }

    async fn diff_from(
        &self,
        workspace_root: &Path,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<String, String> {
        if self.disabled || snapshot_id == "disabled" {
            return Ok(String::new());
        }
        let manifest_raw =
            fs::read_to_string(self.manifest_path(workspace_root, session_id, snapshot_id))
                .map_err(|e| e.to_string())?;
        let manifest: SnapshotManifest =
            serde_json::from_str(&manifest_raw).map_err(|e| e.to_string())?;
        let blobs = self.blobs_dir(workspace_root, session_id);
        let mut out = String::new();
        for (rel, hash) in &manifest.files {
            let Ok(blob_raw) = fs::read(blobs.join(hash)) else {
                continue;
            };
            let Ok(old_bytes) = zstd::stream::decode_all(blob_raw.as_slice()) else {
                continue;
            };
            let old_s = String::from_utf8_lossy(&old_bytes);
            let new_s = fs::read_to_string(workspace_root.join(rel)).unwrap_or_default();
            if old_s != new_s {
                let input = InternedInput::new(old_s.as_ref(), new_s.as_str());
                let diff = Diff::compute(Algorithm::Histogram, &input);
                let u = diff
                    .unified_diff(
                        &imara_diff::BasicLineDiffPrinter(&input.interner),
                        UnifiedDiffConfig::default(),
                        &input,
                    )
                    .to_string();
                out.push_str(&format!("--- a/{rel}\n+++ b/{rel}\n{u}\n"));
            }
        }
        Ok(out)
    }
}

/// 3-Phase Revert (`stage` -> `clear` -> `commit`) coordinating shadow snapshot restore with `revert_staged_seq`.
pub async fn revert_stage(
    store: &EventStore,
    snap: &dyn SnapshotEngine,
    workspace_root: &Path,
    session_id: &str,
    target_seq: i64,
    target_snapshot_id: &str,
) -> Result<String, String> {
    let orig_snap = snap.capture(workspace_root, session_id).await?;
    snap.restore(workspace_root, session_id, target_snapshot_id)
        .await?;
    let sid = session_id.to_string();
    let orig_clone = orig_snap.clone();
    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "UPDATE session SET revert_staged_seq = ?1, revert_orig_snap = ?2 WHERE id = ?3",
                params![target_seq, orig_clone, sid],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(orig_snap)
}

pub async fn revert_clear(
    store: &EventStore,
    snap: &dyn SnapshotEngine,
    workspace_root: &Path,
    session_id: &str,
) -> Result<(), String> {
    let sess = store
        .reader
        .get_session(session_id)?
        .ok_or_else(|| format!("session `{session_id}` not found"))?;
    if let Some(orig) = sess.revert_orig_snap {
        snap.restore(workspace_root, session_id, &orig).await?;
    }
    let sid = session_id.to_string();
    store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute(
                "UPDATE session SET revert_staged_seq = NULL, revert_orig_snap = NULL WHERE id = ?1",
                params![sid],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::Value::Null)
        })
        .await?;
    Ok(())
}

pub async fn revert_commit(store: &EventStore, session_id: &str) -> Result<usize, String> {
    let sess = store
        .reader
        .get_session(session_id)?
        .ok_or_else(|| format!("session `{session_id}` not found"))?;
    let Some(limit_seq) = sess.revert_staged_seq else {
        return Ok(0);
    };
    let sid = session_id.to_string();
    let deleted = store
        .bus
        .writer()
        .exec_tx(move |tx| {
            let n = tx
                .execute(
                    "DELETE FROM session_message WHERE session_id = ?1 AND seq > ?2",
                    params![sid, limit_seq],
                )
                .map_err(|e| e.to_string())?;
            tx.execute(
                "UPDATE session SET revert_staged_seq = NULL, revert_orig_snap = NULL WHERE id = ?1",
                params![sid],
            )
            .map_err(|e| e.to_string())?;
            Ok(serde_json::json!(n))
        })
        .await?;
    Ok(deleted.as_u64().unwrap_or(0) as usize)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{
        ContentPart, InteractionMode, Message, Role, RuntimeMode, SessionEvent,
    };

    #[tokio::test]
    async fn three_phase_revert_stage_clear_commit_restores_disk_and_history() {
        let ws = tempfile::tempdir().unwrap();
        let snap_dir = tempfile::tempdir().unwrap();
        let store = EventStore::open(snap_dir.path().join("snap.sqlite")).unwrap();
        let engine = ShadowSnapshotEngine::new(snap_dir.path().join("snaps"), false);
        let sid = "s_rev".to_string();

        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: sid.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "rev".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: ws.path().to_string_lossy().into_owned(),
                    workspace_root: ws.path().to_string_lossy().into_owned(),
                    runtime_mode: RuntimeMode::Auto,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();

        let f1 = ws.path().join("a.txt");
        fs::write(&f1, "v1").unwrap();
        let s1 = engine.capture(ws.path(), &sid).await.unwrap();
        let r1 = store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: sid.clone(),
                    message: Message {
                        id: "m1".into(),
                        parent_msg_id: None,
                        role: Role::Assistant,
                        parts: vec![ContentPart::Text {
                            text: "step 1".into(),
                            provider_meta: None,
                        }],
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: Some(s1.clone()),
                    changed_files: vec!["a.txt".into()],
                },
                None,
            )
            .await
            .unwrap();

        fs::write(&f1, "v2").unwrap();
        store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: sid.clone(),
                    message: Message {
                        id: "m2".into(),
                        parent_msg_id: Some("m1".into()),
                        role: Role::Assistant,
                        parts: vec![ContentPart::Text {
                            text: "step 2".into(),
                            provider_meta: None,
                        }],
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec!["a.txt".into()],
                },
                None,
            )
            .await
            .unwrap();

        // Phase 1: stage back to s1 -> disk shows v1
        revert_stage(&store, &engine, ws.path(), &sid, r1.seq, &s1)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&f1).unwrap(), "v1");

        // Phase 2: clear (undo the undo) -> disk back to v2
        revert_clear(&store, &engine, ws.path(), &sid)
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&f1).unwrap(), "v2");

        // Phase 3: stage + commit -> deletes seq > r1.seq permanently
        revert_stage(&store, &engine, ws.path(), &sid, r1.seq, &s1)
            .await
            .unwrap();
        let deleted = revert_commit(&store, &sid).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.reader.list_all_messages(&sid).unwrap().len(), 1);
    }
}
