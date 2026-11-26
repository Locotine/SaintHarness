use saint_harness_schema::{EditDraft, EditRequest, EditStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrReplacePayload {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// 3-tier LF-view string replacement: (1) exact -> (2) 1-to-1 unicode punctuation normalize -> (3) trim_end per line.
/// On miss, computes Hough-voting `missHint` (distinct lines, `votes * 2 >= wanted`, capped at 12 lines).
pub fn apply_str_replace(lf_text: &str, payload: &StrReplacePayload) -> Result<String, String> {
    if payload.old_string.is_empty() {
        return Err("old_string must not be empty".to_string());
    }
    let old_lf = payload.old_string.replace("\r\n", "\n");
    let new_lf = payload.new_string.replace("\r\n", "\n");

    // Tier 1: Exact match
    if let Some(res) = try_exact(lf_text, &old_lf, &new_lf, payload.replace_all)? {
        return Ok(res);
    }

    // Tier 2: 1-to-1 Unicode punctuation/quote normalization
    let norm_src = unicode_normalize_1to1(lf_text);
    let norm_old = unicode_normalize_1to1(&old_lf);
    if let Some(res) =
        try_mapped_slice(lf_text, &norm_src, &norm_old, &new_lf, payload.replace_all)?
    {
        return Ok(res);
    }

    // Tier 3: Line-wise trim_end match
    if let Some(res) = try_trim_end_lines(lf_text, &old_lf, &new_lf, payload.replace_all)? {
        return Ok(res);
    }

    let hint = hough_voting_miss_hint(lf_text, &old_lf);
    Err(format!(
        "str_replace failed: target block not found. Closest region (missHint):\n{hint}"
    ))
}

fn try_exact(src: &str, old: &str, new: &str, replace_all: bool) -> Result<Option<String>, String> {
    let matches: Vec<_> = src.match_indices(old).collect();
    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() > 1 && !replace_all {
        return Err(format!(
            "str_replace ambiguous: found {} occurrences (set replace_all=true or add context)",
            matches.len()
        ));
    }
    Ok(Some(if replace_all {
        src.replace(old, new)
    } else {
        src.replacen(old, new, 1)
    }))
}

fn unicode_normalize_1to1(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{00A0}' => ' ',
            other => other,
        })
        .collect()
}

fn try_mapped_slice(
    orig: &str,
    norm_src: &str,
    norm_old: &str,
    new: &str,
    replace_all: bool,
) -> Result<Option<String>, String> {
    let norm_matches: Vec<usize> = norm_src.match_indices(norm_old).map(|(i, _)| i).collect();
    if norm_matches.is_empty() {
        return Ok(None);
    }
    if norm_matches.len() > 1 && !replace_all {
        return Err(format!(
            "str_replace ambiguous after unicode normalization ({} matches)",
            norm_matches.len()
        ));
    }
    let orig_chars: Vec<char> = orig.chars().collect();
    let norm_chars_to_byte: Vec<usize> = orig.char_indices().map(|(b, _)| b).collect();
    let norm_byte_to_char: HashMap<usize, usize> = norm_src
        .char_indices()
        .enumerate()
        .map(|(ci, (bi, _))| (bi, ci))
        .collect();
    let needle_char_len = norm_old.chars().count();

    let mut out = orig.to_string();
    for &start_norm_byte in norm_matches.iter().rev() {
        let Some(&start_ci) = norm_byte_to_char.get(&start_norm_byte) else {
            continue;
        };
        let end_ci = start_ci + needle_char_len;
        let orig_start_byte = *norm_chars_to_byte.get(start_ci).unwrap_or(&orig.len());
        let orig_end_byte = if end_ci >= orig_chars.len() {
            orig.len()
        } else {
            norm_chars_to_byte[end_ci]
        };
        out.replace_range(orig_start_byte..orig_end_byte, new);
    }
    Ok(Some(out))
}

fn try_trim_end_lines(
    src: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<Option<String>, String> {
    let trailing = src.ends_with('\n');
    let src_lines: Vec<&str> = src.strip_suffix('\n').unwrap_or(src).split('\n').collect();
    let old_lines: Vec<&str> = old.strip_suffix('\n').unwrap_or(old).split('\n').collect();
    if old_lines.is_empty() || old_lines.len() > src_lines.len() {
        return Ok(None);
    }

    let mut hits = Vec::new();
    for start in 0..=src_lines.len() - old_lines.len() {
        let matched = old_lines
            .iter()
            .enumerate()
            .all(|(i, ol)| src_lines[start + i].trim_end() == ol.trim_end());
        if matched {
            hits.push(start);
        }
    }
    if hits.is_empty() {
        return Ok(None);
    }
    if hits.len() > 1 && !replace_all {
        return Err(format!(
            "str_replace ambiguous after trim_end matching ({} matches)",
            hits.len()
        ));
    }

    let new_lines: Vec<String> = new
        .strip_suffix('\n')
        .unwrap_or(new)
        .split('\n')
        .map(ToString::to_string)
        .collect();
    let mut lines: Vec<String> = src_lines.into_iter().map(ToString::to_string).collect();
    for start in hits.into_iter().rev() {
        lines.splice(start..start + old_lines.len(), new_lines.clone());
    }
    let mut result = lines.join("\n");
    if trailing {
        result.push('\n');
    }
    Ok(Some(result))
}

/// Hough-voting miss hint: distinct trimmed lines vote on candidate start offsets;
/// accepts best offset when `votes * 2 >= wanted`, showing at most 12 lines.
pub fn hough_voting_miss_hint(src: &str, old: &str) -> String {
    let src_lines: Vec<&str> = src.lines().collect();
    let old_lines: Vec<&str> = old.lines().collect();
    if src_lines.is_empty() || old_lines.is_empty() {
        return "(file or target is empty)".to_string();
    }

    let mut src_positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, line) in src_lines.iter().enumerate() {
        let t = line.trim();
        if !t.is_empty() {
            src_positions.entry(t).or_default().push(idx);
        }
    }

    let mut votes: HashMap<isize, usize> = HashMap::new();
    let mut wanted = 0usize;
    for (needle_idx, line) in old_lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(positions) = src_positions.get(t)
            && positions.len() <= 3
        {
            wanted += 1;
            for &pos in positions {
                let offset = (pos as isize) - (needle_idx as isize);
                *votes.entry(offset).or_insert(0) += 1;
            }
        }
    }

    let best = votes.into_iter().max_by_key(|(_, v)| *v);
    let Some((best_offset, best_votes)) = best else {
        return "(no matching lines found)".to_string();
    };
    if wanted == 0 || best_votes * 2 < wanted {
        return "(no sufficiently similar region found)".to_string();
    }

    let start = best_offset.max(0) as usize;
    let end = (start + old_lines.len().min(12))
        .min(src_lines.len())
        .min(start + 12);
    let mut out = String::new();
    for (i, line) in src_lines[start..end].iter().enumerate() {
        out.push_str(&format!("{:>4} | {line}\n", start + i + 1));
    }
    out
}

pub struct StrReplaceStrategy;

impl EditStrategy for StrReplaceStrategy {
    fn name(&self) -> &str {
        "str_replace"
    }

    fn compute_draft(&self, original: &str, request: &EditRequest) -> Result<EditDraft, String> {
        let payload: StrReplacePayload = serde_json::from_value(request.payload.clone())
            .map_err(|e| format!("invalid str_replace payload: {e}"))?;
        let updated = apply_str_replace(original, &payload)?;
        Ok(EditDraft {
            path: request.path.clone(),
            original_text: original.to_string(),
            updated_text: updated,
            strategy_name: self.name().to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tiers_and_hough_voting_miss_hint_work() {
        let src =
            "fn compute() {\n    let msg = \"hello\";   \n    let x = 1;\n    let y = 2;\n}\n";
        let out = apply_str_replace(
            src,
            &StrReplacePayload {
                old_string: "    let msg = \u{201C}hello\u{201D};   ".into(),
                new_string: "    let msg = \"world\";".into(),
                replace_all: false,
            },
        )
        .unwrap();
        assert!(out.contains("\"world\""));

        let err = apply_str_replace(
            src,
            &StrReplacePayload {
                old_string:
                    "fn compute() {\n    let msg = \"WRONG\";\n    let x = 1;\n    let y = 2;\n}"
                        .into(),
                new_string: "x".into(),
                replace_all: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("missHint"));
        assert!(err.contains("let x = 1;"));
    }
}
