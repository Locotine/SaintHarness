use chrono::{Duration, Utc};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;

pub const DEFAULT_HEAD_BYTES: usize = 16 * 1024;
pub const DEFAULT_TAIL_BYTES: usize = 16 * 1024;
pub const SPILL_RETENTION_DAYS: i64 = 7;

#[derive(Debug, Clone)]
pub struct TruncatedOutput {
    pub text: String,
    pub was_truncated: bool,
    pub spill_path: Option<PathBuf>,
    pub raw_bytes: usize,
}

/// Grapheme-safe head+tail bookend truncation (`16 KiB + 16 KiB`), optionally spilling raw output as zstd.
pub fn truncate_bookend_with_spill(
    input: &str,
    head_bytes: usize,
    tail_bytes: usize,
    spill_dir: Option<&Path>,
    spill_id: Option<&str>,
) -> TruncatedOutput {
    let raw_bytes = input.len();
    let spill_path = match (spill_dir, spill_id) {
        (Some(dir), Some(id)) => write_zstd_spill(dir, id, input.as_bytes()).ok(),
        _ => None,
    };

    if raw_bytes <= head_bytes.saturating_add(tail_bytes) {
        return TruncatedOutput {
            text: input.to_string(),
            was_truncated: false,
            spill_path,
            raw_bytes,
        };
    }

    let head = take_prefix_grapheme_safe(input, head_bytes);
    let tail = take_suffix_grapheme_safe(input, tail_bytes);
    let omitted = raw_bytes.saturating_sub(head.len() + tail.len());

    let marker = match &spill_path {
        Some(p) => format!(
            "\n... [{omitted} bytes truncated; full raw output spilled to {}] ...\n",
            p.display()
        ),
        None => format!("\n... [{omitted} bytes truncated] ...\n"),
    };

    let text = format!("{head}{marker}{tail}");
    TruncatedOutput {
        text,
        was_truncated: true,
        spill_path,
        raw_bytes,
    }
}

pub fn take_prefix_grapheme_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = 0usize;
    for g in s.graphemes(true) {
        if end + g.len() > max_bytes {
            break;
        }
        end += g.len();
    }
    &s[..end]
}

pub fn take_suffix_grapheme_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut taken = 0usize;
    let mut start = s.len();
    for g in s.graphemes(true).rev() {
        if taken + g.len() > max_bytes {
            break;
        }
        taken += g.len();
        start -= g.len();
    }
    &s[start..]
}

pub fn write_zstd_spill(dir: &Path, id: &str, raw: &[u8]) -> Result<PathBuf, std::io::Error> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{id}.out.zst"));
    let compressed = zstd::stream::encode_all(raw, 3)?;
    let mut f = fs::File::create(&path)?;
    f.write_all(&compressed)?;
    Ok(path)
}

pub fn read_zstd_spill(path: &Path) -> Result<Vec<u8>, std::io::Error> {
    let data = fs::read(path)?;
    zstd::stream::decode_all(data.as_slice())
}

pub fn prune_expired_spills(dir: &Path, retention_days: i64) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let cutoff = Utc::now() - Duration::days(retention_days);
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("zst") {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
        {
            let dt: chrono::DateTime<Utc> = modified.into();
            if dt < cutoff && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_safe_truncation_preserves_vietnamese_and_emoji_and_spills_zstd() {
        let tmp = tempfile::tempdir().unwrap();
        let unit = "Tiếng Việt 🚀 ";
        let big = unit.repeat(4000);
        let res = truncate_bookend_with_spill(&big, 512, 512, Some(tmp.path()), Some("job1"));
        assert!(res.was_truncated);
        assert!(res.text.is_char_boundary(0));
        assert!(res.text.contains("Tiếng Việt 🚀"));
        let spill = res.spill_path.expect("spill file written");
        let restored = read_zstd_spill(&spill).unwrap();
        assert_eq!(restored, big.as_bytes());
    }
}
