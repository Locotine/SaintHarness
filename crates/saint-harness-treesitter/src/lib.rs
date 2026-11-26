pub mod grammars;
pub mod outline;
pub mod repomap;
pub mod syntax;

pub use grammars::*;
pub use outline::*;
pub use repomap::*;
pub use syntax::*;

use saint_harness_schema::{CodeOutliner, DiagnosticItem, RepoMapper};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct TreeSitterEngine;

impl CodeOutliner for TreeSitterEngine {
    fn outline(&self, path: &Path, source: &str) -> Result<Option<String>, String> {
        outline_source(path, source)
    }

    fn check_syntax(&self, path: &Path, source: &str) -> Vec<DiagnosticItem> {
        check_syntax_fast(path, source)
    }
}

impl RepoMapper for TreeSitterEngine {
    fn build_repo_map(
        &self,
        workspace_root: &Path,
        budget_tokens: usize,
        focus_files: &[PathBuf],
    ) -> Result<String, String> {
        generate_repo_map(workspace_root, budget_tokens, focus_files)
    }
}
