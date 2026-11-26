use std::path::Path;

/// Pure Rust `gix` repository introspection (branch name, HEAD commit, status) with 0 subprocess forks.
pub fn current_git_branch(workspace_root: &Path) -> Option<String> {
    #[cfg(feature = "git-native")]
    {
        let repo = gix::discover(workspace_root).ok()?;
        let head = repo.head().ok()?;
        if let Some(name) = head.referent_name() {
            return Some(name.shorten().to_string());
        }
        head.id().map(|id| id.to_hex_with_len(7).to_string())
    }
    #[cfg(not(feature = "git-native"))]
    {
        let _ = workspace_root;
        None
    }
}
