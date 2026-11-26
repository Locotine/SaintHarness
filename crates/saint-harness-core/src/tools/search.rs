use async_trait::async_trait;
use globset::Glob;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;
use saint_harness_schema::{Tool, ToolContext, ToolDef, ToolFailure, ToolOutcome, ToolResult};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

const WINDOW_CHARS: usize = 160;
const MAX_MATCHES: usize = 200;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "grep".to_string(),
            description: "Fast regex code search respecting .gitignore, returning compact 160-char match windows grouped by file.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "glob": { "type": "string" }
                },
                "required": ["pattern"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "grep"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(pattern) = arguments.get("pattern").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "grep requires string parameter `pattern`".to_string(),
            });
        };
        let root = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| {
                let pb = Path::new(p);
                if pb.is_absolute() {
                    pb.to_path_buf()
                } else {
                    ctx.cwd.join(pb)
                }
            })
            .unwrap_or_else(|| ctx.workspace_root.clone());
        let glob_filter = arguments
            .get("glob")
            .and_then(|v| v.as_str())
            .and_then(|g| Glob::new(g).ok())
            .map(|g| g.compile_matcher());

        let matcher = match RegexMatcherBuilder::new().build(pattern) {
            Ok(m) => m,
            Err(e) => {
                return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                    message: format!("invalid regex `{pattern}`: {e}"),
                });
            }
        };

        let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut total_hits = 0usize;

        for entry in WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build()
            .flatten()
        {
            if total_hits >= MAX_MATCHES {
                break;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let rel = path
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(ref gm) = glob_filter
                && !gm.is_match(&rel)
            {
                continue;
            }

            let mut searcher = SearcherBuilder::new()
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .line_number(true)
                .build();
            let mut file_hits = Vec::new();
            let _ = searcher.search_path(
                &matcher,
                path,
                UTF8(|line_num, line| {
                    if total_hits + file_hits.len() >= MAX_MATCHES {
                        return Ok(false);
                    }
                    let snippet = clip_window_around(line.trim_end(), pattern, WINDOW_CHARS);
                    file_hits.push(format!("  L{line_num}: {snippet}"));
                    Ok(true)
                }),
            );
            if !file_hits.is_empty() {
                total_hits += file_hits.len();
                grouped.insert(rel, file_hits);
            }
        }

        if grouped.is_empty() {
            return ToolOutcome::Completed(ToolResult::text("No matches found."));
        }
        let mut out = String::new();
        for (file, hits) in grouped {
            out.push_str(&file);
            out.push_str(":\n");
            for h in hits {
                out.push_str(&h);
                out.push('\n');
            }
        }
        ToolOutcome::Completed(ToolResult::text(out))
    }
}

fn clip_window_around(line: &str, needle_hint: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    let hit_char_idx = line
        .find(needle_hint)
        .map(|byte_idx| line[..byte_idx].chars().count())
        .unwrap_or(0);
    let half = max_chars / 2;
    let start = hit_char_idx.saturating_sub(half);
    let end = (start + max_chars).min(chars.len());
    let slice: String = chars[start..end].iter().collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < chars.len() { "…" } else { "" };
    format!("{prefix}{slice}{suffix}")
}

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "glob".to_string(),
            description: "Find files matching a glob pattern respecting .gitignore.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["pattern"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "glob"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(pat) = arguments.get("pattern").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "glob requires string parameter `pattern`".to_string(),
            });
        };
        let matcher = match Glob::new(pat) {
            Ok(g) => g.compile_matcher(),
            Err(e) => {
                return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                    message: format!("invalid glob `{pat}`: {e}"),
                });
            }
        };
        let root = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| ctx.cwd.join(p))
            .unwrap_or_else(|| ctx.workspace_root.clone());

        let mut matches = Vec::new();
        for entry in WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .build()
            .flatten()
        {
            if !entry.path().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .replace('\\', "/");
            if matcher.is_match(&rel) {
                matches.push(rel);
                if matches.len() >= 500 {
                    break;
                }
            }
        }
        matches.sort();
        ToolOutcome::Completed(ToolResult::text(matches.join("\n")))
    }
}
