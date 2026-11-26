use crate::store::EventStore;
use async_trait::async_trait;
use base64::Engine;
use ignore::WalkBuilder;
use imara_diff::{Algorithm, Diff, InternedInput, UnifiedDiffConfig};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use saint_harness_schema::{
    CodeOutliner, SessionEvent, Tool, ToolContext, ToolDef, ToolFailure, ToolOutcome, ToolResult,
    ToolResultBlock,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";
const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;

pub struct ReadTool {
    store: EventStore,
    outliner: Option<Arc<dyn CodeOutliner>>,
    outline_threshold_lines: usize,
    delta_reread: bool,
    read_epoch: Arc<AtomicU64>,
}

impl ReadTool {
    pub fn new(
        store: EventStore,
        outliner: Option<Arc<dyn CodeOutliner>>,
        outline_threshold_lines: usize,
        delta_reread: bool,
        read_epoch: Arc<AtomicU64>,
    ) -> Self {
        Self {
            store,
            outliner,
            outline_threshold_lines,
            delta_reread,
            read_epoch,
        }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "read".to_string(),
            description: "Read file lines with Hashline anchors, automatic AST outline for large files, and Delta Re-Read.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative or absolute file path" },
                    "offset": { "type": "integer", "description": "1-based start line (negative counts from EOF)" },
                    "limit": { "type": "integer", "description": "Maximum number of lines to return" },
                    "full": { "type": "boolean", "description": "Force full line view instead of outline or delta" }
                },
                "required": ["path"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "read"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(raw_path) = arguments.get("path").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "read requires string parameter `path`".to_string(),
            });
        };
        let path = resolve_path(&ctx.cwd, raw_path);
        if !path.exists() {
            let suggestions = fuzzy_did_you_mean(&ctx.workspace_root, raw_path, 5);
            let hint = if suggestions.is_empty() {
                String::new()
            } else {
                format!(" Did you mean: {}?", suggestions.join(", "))
            };
            return ToolOutcome::RecoverableError(ToolFailure::RuntimeError {
                message: format!("file not found: {}.{}", raw_path, hint),
            });
        }

        let Ok(bytes) = fs::read(&path) else {
            return ToolOutcome::RecoverableError(ToolFailure::RuntimeError {
                message: format!("failed to read file: {}", path.display()),
            });
        };

        // Image files return Media content; other binary files containing NUL bytes are rejected.
        if let Some(mime) = image_mime_for_path(&path) {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return ToolOutcome::Completed(ToolResult {
                content: vec![ToolResultBlock::Media {
                    mime_type: mime.to_string(),
                    base64_data: b64,
                }],
                raw_bytes: bytes.len(),
                condensed_bytes: bytes.len(),
                metadata: None,
            });
        }
        if memchr::memchr(0, &bytes).is_some() {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: format!(
                    "refusing to read binary file containing NUL bytes: {}",
                    path.display()
                ),
            });
        }

        let text = String::from_utf8_lossy(&bytes).into_owned();
        let canonical = fs::canonicalize(&path)
            .unwrap_or_else(|_| path.clone())
            .to_string_lossy()
            .into_owned();
        let blake3_hex = blake3::hash(text.as_bytes()).to_hex().to_string();
        let lines: Vec<&str> = text.lines().collect();
        let total_lines = lines.len();
        let offset_arg = arguments.get("offset").and_then(|v| v.as_i64());
        let limit_arg = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);
        let force_full = arguments
            .get("full")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let cur_epoch = self.read_epoch.load(Ordering::Relaxed);

        // Delta Re-Read (read_diff): on whole-file re-reads within the same epoch, return [Unchanged] or compact diff.
        if self.delta_reread
            && !force_full
            && offset_arg.is_none()
            && limit_arg.is_none()
            && let Ok(Some(prev)) = self
                .store
                .reader
                .get_file_read_state(&ctx.session_id, &canonical)
            && prev.read_epoch == cur_epoch
        {
            if prev.last_blake3 == blake3_hex {
                return ToolOutcome::Completed(ToolResult::text(format!(
                    "[Unchanged · delta 0B] {} ({total_lines} lines)",
                    raw_path
                )));
            }
            if let Some(ref zst_blob) = prev.snapshot_zst
                && let Ok(prev_bytes) = zstd::stream::decode_all(zst_blob.as_slice())
            {
                let prev_text = String::from_utf8_lossy(&prev_bytes);
                let input = InternedInput::new(prev_text.as_ref(), text.as_str());
                let diff = Diff::compute(Algorithm::Histogram, &input);
                let unified = diff
                    .unified_diff(
                        &imara_diff::BasicLineDiffPrinter(&input.interner),
                        UnifiedDiffConfig::default(),
                        &input,
                    )
                    .to_string();
                self.record_read_state(
                    &ctx.session_id,
                    canonical,
                    blake3_hex,
                    total_lines,
                    &text,
                    cur_epoch,
                )
                .await;
                return ToolOutcome::Completed(ToolResult::text(format!(
                    "[Delta Re-Read: {}]\n{}",
                    raw_path, unified
                )));
            }
        }

        self.record_read_state(
            &ctx.session_id,
            canonical,
            blake3_hex,
            total_lines,
            &text,
            cur_epoch,
        )
        .await;

        // Automatic AST Outline for large whole-file reads when outliner is wired.
        if !force_full
            && offset_arg.is_none()
            && limit_arg.is_none()
            && total_lines >= self.outline_threshold_lines
            && let Some(ref outliner) = self.outliner
            && let Ok(Some(outline)) = outliner.outline(&path, &text)
        {
            return ToolOutcome::Completed(ToolResult::text(format!(
                "[AST Outline: {raw_path} ({total_lines} lines -> {} lines). Pass offset/limit or full=true for body.]\n{outline}",
                outline.lines().count()
            )));
        }

        // Compute 0-based start/end slice supporting negative offsets & past-EOF tail (last 100 lines).
        let start_idx = match offset_arg {
            None => 0usize,
            Some(off) if off < 0 => total_lines.saturating_sub(off.unsigned_abs() as usize),
            Some(off) => {
                let zero = off.saturating_sub(1) as usize;
                if zero >= total_lines {
                    total_lines.saturating_sub(100)
                } else {
                    zero
                }
            }
        };
        let limit = limit_arg.unwrap_or(400);
        let end_idx = (start_idx + limit).min(total_lines);

        let mut out = String::new();
        for idx in start_idx..end_idx {
            let prev = if idx > 0 {
                lines.get(idx - 1).copied()
            } else {
                None
            };
            let next = lines.get(idx + 1).copied();
            let h = compute_hash(lines[idx], prev, next);
            out.push_str(&format!("{}#{h}|{}\n", idx + 1, lines[idx]));
        }
        ToolOutcome::Completed(ToolResult::text(out))
    }
}

impl ReadTool {
    async fn record_read_state(
        &self,
        session_id: &str,
        canonical_path: String,
        blake3_hex: String,
        line_count: usize,
        text: &str,
        read_epoch: u64,
    ) {
        let snapshot_zst = if text.len() <= MAX_SNAPSHOT_BYTES {
            zstd::stream::encode_all(text.as_bytes(), 3).ok()
        } else {
            None
        };
        let _ = self
            .store
            .bus
            .publish_durable(
                SessionEvent::FileReadRecorded {
                    session_id: session_id.to_string(),
                    canonical_path,
                    blake3_hex,
                    line_count,
                    snapshot_zst,
                    read_epoch,
                },
                None,
            )
            .await;
    }
}

fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn image_mime_for_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

fn compute_hash(line: &str, prev: Option<&str>, next: Option<&str>) -> String {
    let norm = line.trim();
    let mut h: u32 = 0x811c_9dc5;
    let feed = |state: &mut u32, bytes: &[u8]| {
        for &b in bytes {
            *state ^= u32::from(b);
            *state = state.wrapping_mul(0x0100_0193);
        }
    };
    feed(&mut h, norm.as_bytes());
    if norm.len() <= 2 || !norm.chars().any(char::is_alphanumeric) {
        feed(&mut h, &[0x1f]);
        if let Some(p) = prev {
            feed(&mut h, p.trim().as_bytes());
        }
        feed(&mut h, &[0x1e]);
        if let Some(n) = next {
            feed(&mut h, n.trim().as_bytes());
        }
    }
    let mut s = String::with_capacity(2);
    s.push(ALPHABET[((h >> 4) & 0x0f) as usize] as char);
    s.push(ALPHABET[(h & 0x0f) as usize] as char);
    s
}

/// Uses `nucleo-matcher` to rank fuzzy "Did you mean?" candidate file paths.
pub fn fuzzy_did_you_mean(workspace_root: &Path, query: &str, limit: usize) -> Vec<String> {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pat = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, String)> = Vec::new();

    for entry in WalkBuilder::new(workspace_root)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(8))
        .build()
        .flatten()
    {
        if !entry.path().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(workspace_root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        buf.clear();
        let hay = Utf32Str::new(&rel, &mut buf);
        if let Some(score) = pat.score(hay, &mut matcher) {
            scored.push((score, rel));
        }
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, p)| p).collect()
}
