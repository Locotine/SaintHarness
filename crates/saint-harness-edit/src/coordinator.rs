use crate::ast_edit::AstEditStrategy;
use crate::endings::{compute_unified_diff, normalize_to_lf, restore_endings_histogram};
use crate::hashline::HashlineStrategy;
use crate::str_replace::StrReplaceStrategy;
use crate::symbol_edit::ReplaceSymbolStrategy;
use async_trait::async_trait;
use dashmap::DashMap;
use saint_harness_schema::{
    CodeOutliner, DiagnosticItem, EditRequest, EditStrategy, FastLintPort, FileWritePort,
    FileWriteResult, LspPort,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Single file-write coordinator shared across all 4 edit strategies (`hashline`, `str_replace`,
/// `replace_symbol`, `ast_edit`). Owns keyed file locking, BOM + Histogram line-ending preservation,
/// atomic temp+sync+rename writes preserving POSIX mode, and post-write syntax/fast-lint/LSP checks.
pub struct FileWriteCoordinator {
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
    strategies: BTreeMap<String, Arc<dyn EditStrategy>>,
    outliner: Option<Arc<dyn CodeOutliner>>,
    fast_lint: Option<Arc<dyn FastLintPort>>,
    lsp: Option<Arc<dyn LspPort>>,
}

impl Default for FileWriteCoordinator {
    fn default() -> Self {
        Self::new(None, None, None)
    }
}

impl FileWriteCoordinator {
    pub fn new(
        outliner: Option<Arc<dyn CodeOutliner>>,
        fast_lint: Option<Arc<dyn FastLintPort>>,
        lsp: Option<Arc<dyn LspPort>>,
    ) -> Self {
        let mut strategies: BTreeMap<String, Arc<dyn EditStrategy>> = BTreeMap::new();
        strategies.insert("hashline".to_string(), Arc::new(HashlineStrategy));
        strategies.insert("str_replace".to_string(), Arc::new(StrReplaceStrategy));
        strategies.insert(
            "replace_symbol".to_string(),
            Arc::new(ReplaceSymbolStrategy),
        );
        strategies.insert("ast_edit".to_string(), Arc::new(AstEditStrategy));

        Self {
            locks: DashMap::new(),
            strategies,
            outliner,
            fast_lint,
            lsp,
        }
    }

    fn file_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.locks
            .entry(canonical)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn collect_diagnostics(&self, path: &Path, content: &str) -> Vec<DiagnosticItem> {
        let mut diags = Vec::new();
        if let Some(ref lint) = self.fast_lint {
            diags.extend(lint.lint_file(path, content).await);
        }
        if let Some(ref lsp) = self.lsp {
            diags.extend(lsp.diagnostics_after_write(path).await);
        }
        if diags.is_empty()
            && let Some(ref outliner) = self.outliner
        {
            diags.extend(outliner.check_syntax(path, content));
        }
        diags
    }
}

fn atomic_write_preserve_mode(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("failed to create parent dir: {e}"))?;

    let existing_perms = fs::metadata(path).ok().map(|m| m.permissions());
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("failed to create temp file: {e}"))?;
    tmp.write_all(bytes)
        .map_err(|e| format!("failed to write temp file: {e}"))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| format!("failed to sync temp file: {e}"))?;
    if let Some(perms) = existing_perms {
        let _ = fs::set_permissions(tmp.path(), perms);
    }
    tmp.persist(path)
        .map_err(|e| format!("failed to persist atomic rename: {e}"))?;
    Ok(())
}

#[async_trait]
impl FileWritePort for FileWriteCoordinator {
    async fn apply_edit(
        &self,
        strategy_name: &str,
        request: EditRequest,
    ) -> Result<FileWriteResult, String> {
        let strategy = self
            .strategies
            .get(strategy_name)
            .cloned()
            .ok_or_else(|| format!("unknown edit strategy '{strategy_name}'"))?;
        let lock = self.file_lock(&request.path);
        let _guard = lock.lock().await;

        let raw = fs::read_to_string(&request.path)
            .map_err(|e| format!("failed to read {}: {e}", request.path.display()))?;
        let doc = normalize_to_lf(&raw);
        let draft = strategy.compute_draft(&doc.lf_text, &request)?;
        let (unified_diff, lines_added, lines_removed) =
            compute_unified_diff(&doc.lf_text, &draft.updated_text);
        let final_text = restore_endings_histogram(&doc, &draft.updated_text);

        atomic_write_preserve_mode(&request.path, final_text.as_bytes())?;
        let blake3_hex = blake3::hash(final_text.as_bytes()).to_hex().to_string();
        let diagnostics = self
            .collect_diagnostics(&request.path, &draft.updated_text)
            .await;

        Ok(FileWriteResult {
            path: request.path,
            unified_diff,
            lines_added,
            lines_removed,
            blake3_hex,
            diagnostics,
            reformatted: false,
        })
    }

    async fn write_new_file(
        &self,
        path: PathBuf,
        content: String,
    ) -> Result<FileWriteResult, String> {
        let lock = self.file_lock(&path);
        let _guard = lock.lock().await;

        let before = fs::read_to_string(&path).unwrap_or_default();
        let (unified_diff, lines_added, lines_removed) = compute_unified_diff(&before, &content);
        atomic_write_preserve_mode(&path, content.as_bytes())?;
        let blake3_hex = blake3::hash(content.as_bytes()).to_hex().to_string();
        let diagnostics = self.collect_diagnostics(&path, &content).await;

        Ok(FileWriteResult {
            path,
            unified_diff,
            lines_added,
            lines_removed,
            blake3_hex,
            diagnostics,
            reformatted: false,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::hashline::line_hash;
    use serde_json::json;

    #[tokio::test]
    async fn coordinator_writes_atomically_and_preserves_crlf_and_bom() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("demo.rs");
        fs::write(&file, "\u{FEFF}fn a() {\r\n    let x = 1;\r\n}\r\n").unwrap();

        let coord = FileWriteCoordinator::default();
        let h2 = line_hash("    let x = 1;", Some("fn a() {"), Some("}"));
        let res = coord
            .apply_edit(
                "hashline",
                EditRequest {
                    path: file.clone(),
                    payload: json!({
                        "edits": [{
                            "start_anchor": format!("2#{h2}"),
                            "content": "    let x = 42;"
                        }]
                    }),
                },
            )
            .await
            .unwrap();

        assert_eq!(res.lines_added, 1);
        assert_eq!(res.lines_removed, 1);
        let on_disk = fs::read_to_string(&file).unwrap();
        assert_eq!(on_disk, "\u{FEFF}fn a() {\r\n    let x = 42;\r\n}\r\n");
    }
}
