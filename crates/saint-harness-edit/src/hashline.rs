use regex::Regex;
use saint_harness_schema::{EditDraft, EditRequest, EditStrategy};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const ALPHABET: &[u8; 16] = b"ZPMQVRWSNKTXJBYH";

/// FNV-1a 32-bit hash folded into two alphabet characters (`16x16 = 256` buckets).
/// Short or symbol-only lines mix in adjacent trimmed lines to prevent brace/blank collisions.
pub fn line_hash(line: &str, prev: Option<&str>, next: Option<&str>) -> String {
    let norm = line.trim();
    let mut h: u32 = 0x811c_9dc5;
    let feed = |state: &mut u32, bytes: &[u8]| {
        for &b in bytes {
            *state ^= u32::from(b);
            *state = state.wrapping_mul(0x0100_0193);
        }
    };

    feed(&mut h, norm.as_bytes());
    let needs_neighbors = norm.len() <= 2 || !norm.chars().any(char::is_alphanumeric);
    if needs_neighbors {
        feed(&mut h, &[0x1f]);
        if let Some(p) = prev {
            feed(&mut h, p.trim().as_bytes());
        }
        feed(&mut h, &[0x1e]);
        if let Some(n) = next {
            feed(&mut h, n.trim().as_bytes());
        }
    }

    let hi = ((h >> 4) & 0x0f) as usize;
    let lo = (h & 0x0f) as usize;
    let mut s = String::with_capacity(2);
    s.push(ALPHABET[hi] as char);
    s.push(ALPHABET[lo] as char);
    s
}

pub fn format_hashline_view(lf_text: &str, start_line: usize) -> String {
    let lines: Vec<&str> = split_logical_lines(lf_text);
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        let prev = if idx > 0 { Some(lines[idx - 1]) } else { None };
        let next = lines.get(idx + 1).copied();
        let tag = line_hash(line, prev, next);
        let num = start_line + idx;
        out.push_str(&format!("{num}#{tag}|{line}\n"));
    }
    out
}

pub fn split_logical_lines(lf_text: &str) -> Vec<&str> {
    if lf_text.is_empty() {
        return Vec::new();
    }
    let trimmed = lf_text.strip_suffix('\n').unwrap_or(lf_text);
    trimmed.split('\n').collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashlineEditOp {
    /// 1-based anchor string `"42#ZP"` (start line inclusive)
    pub start_anchor: String,
    /// Optional 1-based end anchor `"48#VR"` (inclusive); defaults to single line `start_anchor`.
    #[serde(default)]
    pub end_anchor: Option<String>,
    /// Replacement lines (empty string deletes the range if `delete == true`, otherwise replaces with empty line).
    pub content: String,
    #[serde(default)]
    pub delete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashlinePayload {
    pub edits: Vec<HashlineEditOp>,
}

fn parse_anchor(anchor: &str) -> Result<(usize, String), String> {
    let (num_str, hash_str) = anchor
        .trim()
        .split_once('#')
        .ok_or_else(|| format!("invalid hashline anchor '{anchor}', expected '<line>#<HH>'"))?;
    let line_no = num_str
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("invalid line number in anchor '{anchor}'"))?;
    if line_no == 0 {
        return Err(format!("line number must be >= 1 in anchor '{anchor}'"));
    }
    let hash_clean = hash_str
        .split('|')
        .next()
        .unwrap_or(hash_str)
        .trim()
        .to_ascii_uppercase();
    if hash_clean.len() != 2 {
        return Err(format!(
            "hash part in anchor '{anchor}' must be exactly 2 characters"
        ));
    }
    Ok((line_no, hash_clean))
}

/// Strips accidental `^\d+#[A-Z]{2}\|` prefixes copied by models into replacement content.
pub fn strip_accidental_hashline_prefixes(content: &str) -> String {
    static PREFIX_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        PREFIX_RE.get_or_init(|| Regex::new(r"^\d+#[A-Z]{2}\|").unwrap_or_else(|_| unreachable!()));
    content
        .split('\n')
        .map(|line| re.replace(line, "").into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Validates 100% of anchors, checks non-overlap, sorts bottom-up descending, and applies atomically.
pub fn apply_hash_edits(lf_text: &str, ops: &[HashlineEditOp]) -> Result<String, String> {
    let trailing_newline = lf_text.ends_with('\n');
    let mut lines: Vec<String> = split_logical_lines(lf_text)
        .into_iter()
        .map(ToString::to_string)
        .collect();
    let total = lines.len();

    let mut validated: Vec<(usize, usize, Option<Vec<String>>)> = Vec::with_capacity(ops.len());
    for op in ops {
        let (start_no, start_hash) = parse_anchor(&op.start_anchor)?;
        let (end_no, end_hash) = match &op.end_anchor {
            Some(e) => parse_anchor(e)?,
            None => (start_no, start_hash.clone()),
        };
        if start_no > end_no {
            return Err(format!(
                "invalid edit range: start line {start_no} > end line {end_no}"
            ));
        }

        verify_anchor(&lines, start_no, &start_hash)?;
        if end_no != start_no {
            verify_anchor(&lines, end_no, &end_hash)?;
        }

        let replacement = if op.delete {
            None
        } else {
            let cleaned = strip_accidental_hashline_prefixes(&op.content);
            Some(cleaned.split('\n').map(ToString::to_string).collect())
        };
        validated.push((start_no - 1, end_no - 1, replacement));
    }

    // Check overlap after sorting ascending by start index
    validated.sort_by_key(|(s, e, _)| (*s, *e));
    for pair in validated.windows(2) {
        if pair[0].1 >= pair[1].0 {
            return Err(format!(
                "overlapping hashline edits at lines {}..={} and {}..={}",
                pair[0].0 + 1,
                pair[0].1 + 1,
                pair[1].0 + 1,
                pair[1].1 + 1
            ));
        }
    }

    // Apply bottom-up (descending line number) so earlier indices remain valid.
    validated.reverse();
    for (start_idx, end_idx, replacement) in validated {
        if end_idx >= total {
            return Err(format!(
                "line {} out of bounds (file has {total} lines)",
                end_idx + 1
            ));
        }
        match replacement {
            Some(new_lines) => {
                lines.splice(start_idx..=end_idx, new_lines);
            }
            None => {
                lines.drain(start_idx..=end_idx);
            }
        }
    }

    let mut result = lines.join("\n");
    if trailing_newline && !lines.is_empty() {
        result.push('\n');
    }
    Ok(result)
}

fn verify_anchor(lines: &[String], line_no: usize, expected_hash: &str) -> Result<(), String> {
    let idx = line_no - 1;
    let Some(actual_line) = lines.get(idx) else {
        return Err(format!(
            "StaleAnchorError: line {line_no} is out of range (file has {} lines)",
            lines.len()
        ));
    };
    let prev = if idx > 0 {
        lines.get(idx - 1).map(String::as_str)
    } else {
        None
    };
    let next = lines.get(idx + 1).map(String::as_str);
    let actual_hash = line_hash(actual_line, prev, next);
    if actual_hash != expected_hash {
        let from = idx.saturating_sub(4);
        let to = (idx + 5).min(lines.len());
        let mut snippet = String::new();
        for i in from..to {
            let p = if i > 0 {
                Some(lines[i - 1].as_str())
            } else {
                None
            };
            let n = lines.get(i + 1).map(String::as_str);
            let h = line_hash(&lines[i], p, n);
            snippet.push_str(&format!("{}#{}|{}\n", i + 1, h, lines[i]));
        }
        return Err(format!(
            "StaleAnchorError at line {line_no}: expected #{expected_hash} but found #{actual_hash}. Fresh ±4 context:\n{snippet}"
        ));
    }
    Ok(())
}

pub struct HashlineStrategy;

impl EditStrategy for HashlineStrategy {
    fn name(&self) -> &str {
        "hashline"
    }

    fn compute_draft(&self, original: &str, request: &EditRequest) -> Result<EditDraft, String> {
        let payload: HashlinePayload = serde_json::from_value(request.payload.clone())
            .map_err(|e| format!("invalid hashline payload: {e}"))?;
        let updated = apply_hash_edits(original, &payload.edits)?;
        Ok(EditDraft {
            path: request.path.clone(),
            original_text: original.to_string(),
            updated_text: updated,
            strategy_name: self.name().to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn multi_edit_bottom_up_and_prefix_strip() {
        let src = "fn alpha() {\n    let a = 1;\n}\nfn beta() {\n    let b = 2;\n}\n";
        let lines = split_logical_lines(src);
        let a2 = format!("2#{}", line_hash(lines[1], Some(lines[0]), Some(lines[2])));
        let a5 = format!("5#{}", line_hash(lines[4], Some(lines[3]), Some(lines[5])));

        let updated = apply_hash_edits(
            src,
            &[
                HashlineEditOp {
                    start_anchor: a2,
                    end_anchor: None,
                    content: "2#ZP|    let a = 10;\n    let extra = 20;".to_string(),
                    delete: false,
                },
                HashlineEditOp {
                    start_anchor: a5,
                    end_anchor: None,
                    content: "    let b = 200;".to_string(),
                    delete: false,
                },
            ],
        )
        .unwrap();

        assert!(updated.contains("    let a = 10;\n    let extra = 20;"));
        assert!(updated.contains("    let b = 200;"));
        assert!(!updated.contains("2#ZP|"));
    }

    #[test]
    fn stale_anchor_returns_plus_minus_four_context_and_rejects_overlap() {
        let src = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n";
        let err = apply_hash_edits(
            src,
            &[HashlineEditOp {
                start_anchor: "4#ZZ".to_string(),
                end_anchor: None,
                content: "new".to_string(),
                delete: false,
            }],
        )
        .unwrap_err();
        assert!(err.contains("StaleAnchorError"));
        assert!(err.contains("1#"));
        assert!(err.contains("7#"));

        let lines = split_logical_lines(src);
        let a2 = format!("2#{}", line_hash(lines[1], Some(lines[0]), Some(lines[2])));
        let a3 = format!("3#{}", line_hash(lines[2], Some(lines[1]), Some(lines[3])));
        let a4 = format!("4#{}", line_hash(lines[3], Some(lines[2]), Some(lines[4])));
        let overlap_err = apply_hash_edits(
            src,
            &[
                HashlineEditOp {
                    start_anchor: a2,
                    end_anchor: Some(a4),
                    content: "x".into(),
                    delete: false,
                },
                HashlineEditOp {
                    start_anchor: a3,
                    end_anchor: None,
                    content: "y".into(),
                    delete: false,
                },
            ],
        )
        .unwrap_err();
        assert!(overlap_err.contains("overlapping"));
    }
}
