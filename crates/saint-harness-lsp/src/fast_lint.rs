use async_trait::async_trait;
use saint_harness_schema::{DiagnosticItem, FastLintPort};
use std::path::Path;
use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct FastLintEngine;

#[async_trait]
impl FastLintPort for FastLintEngine {
    async fn lint_file(&self, path: &Path, content: &str) -> Vec<DiagnosticItem> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match ext.as_str() {
            "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => lint_js_ts(path, content),
            "py" => lint_python_ruff(path).await,
            _ => Vec::new(),
        }
    }
}

pub fn lint_js_ts(path: &Path, content: &str) -> Vec<DiagnosticItem> {
    #[cfg(feature = "fast-lint")]
    {
        use oxc_allocator::Allocator;
        use oxc_parser::Parser;
        use oxc_semantic::SemanticBuilder;
        use oxc_span::SourceType;

        let allocator = Allocator::default();
        let source_type = SourceType::from_path(path).unwrap_or_default();
        let parsed = Parser::new(&allocator, content, source_type).parse();
        let path_str = path.to_string_lossy().into_owned();
        let mut out = Vec::new();
        for err in parsed.diagnostics {
            out.push(DiagnosticItem {
                path: path_str.clone(),
                line: 1,
                column: 1,
                severity: "error".to_string(),
                message: err.to_string(),
                source: "oxc".to_string(),
            });
        }
        if out.is_empty() {
            let sem = SemanticBuilder::new()
                .with_check_syntax_error(true)
                .build(&parsed.program);
            for err in sem.diagnostics {
                out.push(DiagnosticItem {
                    path: path_str.clone(),
                    line: 1,
                    column: 1,
                    severity: "error".to_string(),
                    message: err.to_string(),
                    source: "oxc".to_string(),
                });
            }
        }
        out
    }

    #[cfg(not(feature = "fast-lint"))]
    {
        // Lightweight brace balance check when `fast-lint` feature is not enabled.
        let mut depth = 0i32;
        for ch in content.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth != 0 {
            vec![DiagnosticItem {
                path: path.to_string_lossy().into_owned(),
                line: 1,
                column: 1,
                severity: "error".to_string(),
                message: "unbalanced braces in JS/TS source".to_string(),
                source: "fast-lint-fallback".to_string(),
            }]
        } else {
            Vec::new()
        }
    }
}

async fn lint_python_ruff(path: &Path) -> Vec<DiagnosticItem> {
    let output = Command::new("ruff")
        .args(["check", "--output-format", "json"])
        .arg(path)
        .output()
        .await;
    let Ok(out) = output else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return Vec::new();
    };
    items
        .into_iter()
        .take(15)
        .map(|it| DiagnosticItem {
            path: path.to_string_lossy().into_owned(),
            line: it
                .get("location")
                .and_then(|l| l.get("row"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            column: it
                .get("location")
                .and_then(|l| l.get("column"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            severity: "error".to_string(),
            message: it
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("ruff diagnostic")
                .to_string(),
            source: "ruff".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_lint_catches_js_ts_errors_with_or_without_oxc() {
        let broken = lint_js_ts(Path::new("broken.ts"), "function f( { return 1;");
        assert!(!broken.is_empty());

        let clean = lint_js_ts(Path::new("clean.ts"), "export const x: number = 42;\n");
        assert!(clean.is_empty());

        #[cfg(feature = "fast-lint")]
        {
            let dup = lint_js_ts(Path::new("dup.ts"), "const a = 1;\nconst a = 2;\n");
            assert!(
                !dup.is_empty(),
                "oxc semantic check must catch redeclaration"
            );
            assert_eq!(dup[0].source, "oxc");
        }
    }
}
