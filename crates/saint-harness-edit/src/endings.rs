use imara_diff::{Algorithm, Diff, InternedInput, UnifiedDiffConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NormalizedDocument {
    pub had_bom: bool,
    pub dominant_ending: LineEnding,
    pub original_line_endings: Vec<LineEnding>,
    pub had_trailing_newline: bool,
    pub lf_text: String,
}

pub fn normalize_to_lf(raw: &str) -> NormalizedDocument {
    let (had_bom, body) = match raw.strip_prefix('\u{FEFF}') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };

    let mut endings = Vec::new();
    let mut crlf_count = 0usize;
    let mut lf_count = 0usize;
    let mut lf_text = String::with_capacity(body.len());

    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            endings.push(LineEnding::Crlf);
            crlf_count += 1;
            lf_text.push('\n');
            i += 2;
        } else if bytes[i] == b'\n' {
            endings.push(LineEnding::Lf);
            lf_count += 1;
            lf_text.push('\n');
            i += 1;
        } else {
            let ch = body[i..].chars().next().unwrap_or('\0');
            lf_text.push(ch);
            i += ch.len_utf8();
        }
    }

    let dominant_ending = if crlf_count > lf_count {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    let had_trailing_newline = lf_text.ends_with('\n');

    NormalizedDocument {
        had_bom,
        dominant_ending,
        original_line_endings: endings,
        had_trailing_newline,
        lf_text,
    }
}

/// Restores UTF-8 BOM and per-line CRLF/LF endings using `imara-diff` Histogram diff so
/// unchanged lines keep their exact original endings even in mixed-ending files.
pub fn restore_endings_histogram(doc: &NormalizedDocument, updated_lf: &str) -> String {
    let input = InternedInput::new(doc.lf_text.as_str(), updated_lf);
    let diff = Diff::compute(Algorithm::Histogram, &input);

    let new_lines: Vec<&str> = updated_lf.split('\n').collect();
    let has_trailing = updated_lf.ends_with('\n');
    let line_count = if has_trailing {
        new_lines.len().saturating_sub(1)
    } else {
        new_lines.len()
    };

    let mut mapped_endings = vec![doc.dominant_ending; line_count];
    // Walk hunks: unchanged lines outside hunks map 1-to-1 from before -> after.
    let mut before_cursor = 0usize;
    let mut after_cursor = 0usize;
    for hunk in diff.hunks() {
        let b_start = hunk.before.start as usize;
        let a_start = hunk.after.start as usize;
        while before_cursor < b_start && after_cursor < a_start && after_cursor < line_count {
            if let Some(&orig_end) = doc.original_line_endings.get(before_cursor) {
                mapped_endings[after_cursor] = orig_end;
            }
            before_cursor += 1;
            after_cursor += 1;
        }
        before_cursor = hunk.before.end as usize;
        after_cursor = hunk.after.end as usize;
    }
    while after_cursor < line_count && before_cursor < doc.original_line_endings.len() {
        mapped_endings[after_cursor] = doc.original_line_endings[before_cursor];
        before_cursor += 1;
        after_cursor += 1;
    }

    let mut out = String::with_capacity(updated_lf.len() + line_count + 4);
    if doc.had_bom {
        out.push('\u{FEFF}');
    }
    for (idx, line) in new_lines.iter().take(line_count).enumerate() {
        out.push_str(line);
        let is_last = idx + 1 == line_count;
        if !is_last || has_trailing {
            out.push_str(mapped_endings[idx].as_str());
        }
    }
    out
}

pub fn compute_unified_diff(before: &str, after: &str) -> (String, usize, usize) {
    let input = InternedInput::new(before, after);
    let diff = Diff::compute(Algorithm::Histogram, &input);
    let added = diff.count_additions() as usize;
    let removed = diff.count_removals() as usize;
    let unified = diff
        .unified_diff(
            &imara_diff::BasicLineDiffPrinter(&input.interner),
            UnifiedDiffConfig::default(),
            &input,
        )
        .to_string();
    (unified, added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_utf8_bom_and_mixed_crlf_lf_via_histogram_diff() {
        let raw = "\u{FEFF}line1\r\nline2\nline3\r\nline4\r\n";
        let norm = normalize_to_lf(raw);
        assert!(norm.had_bom);
        assert_eq!(norm.dominant_ending, LineEnding::Crlf);
        let updated = "line1\nline2\nline3_edited\nline4\n";
        let restored = restore_endings_histogram(&norm, updated);
        assert!(restored.starts_with('\u{FEFF}'));
        assert_eq!(
            restored,
            "\u{FEFF}line1\r\nline2\nline3_edited\r\nline4\r\n"
        );
    }
}
