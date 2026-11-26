use crate::grammars::parse_source;
use saint_harness_schema::DiagnosticItem;
use std::path::Path;
use tree_sitter::Node;

/// Fast `< 1ms` structural syntax check scanning for tree-sitter `ERROR` and `MISSING` nodes.
pub fn check_syntax_fast(path: &Path, source: &str) -> Vec<DiagnosticItem> {
    let Some((_, tree)) = parse_source(path, source) else {
        return Vec::new();
    };
    let root = tree.root_node();
    if !root.has_error() {
        return Vec::new();
    }
    let path_str = path.to_string_lossy().into_owned();
    let mut diags = Vec::new();
    collect_errors(root, &path_str, &mut diags);
    diags
}

fn collect_errors(node: Node<'_>, path: &str, out: &mut Vec<DiagnosticItem>) {
    if out.len() >= 15 {
        return;
    }
    if node.is_error() || node.is_missing() {
        let pos = node.start_position();
        let message = if node.is_missing() {
            format!("missing syntax token `{}`", node.kind())
        } else {
            "syntax error".to_string()
        };
        out.push(DiagnosticItem {
            path: path.to_string(),
            line: pos.row as u32 + 1,
            column: pos.column as u32 + 1,
            severity: "error".to_string(),
            message,
            source: "tree-sitter".to_string(),
        });
        return;
    }
    if !node.has_error() {
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, path, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    #[test]
    fn detects_missing_semicolon_or_brace_under_1ms() {
        let broken = "fn main() {\n    let x = 1\n    let y = 2;\n";
        let start = Instant::now();
        let diags = check_syntax_fast(&PathBuf::from("broken.rs"), broken);
        let elapsed = start.elapsed();
        assert!(!diags.is_empty());
        assert!(
            elapsed.as_millis() < 50,
            "unexpectedly slow syntax check: {elapsed:?}"
        );

        let clean = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        assert!(check_syntax_fast(&PathBuf::from("clean.rs"), clean).is_empty());
    }
}
