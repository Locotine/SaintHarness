use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct CondensedOutput {
    pub text: String,
    pub raw_bytes: usize,
    pub condensed_bytes: usize,
}

impl CondensedOutput {
    pub fn savings_ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            0.0
        } else {
            1.0 - (self.condensed_bytes as f64 / self.raw_bytes as f64)
        }
    }
}

/// Write-time semantic condenser for CLI and JSON tool outputs (0% prompt cache invalidation).
pub fn condense_cli_output(command: &str, raw: &str) -> CondensedOutput {
    let raw_bytes = raw.len();
    let cr_normalized = simulate_carriage_returns(raw);
    let clean = strip_ansi(&cr_normalized);

    let trimmed = clean.trim();
    if (trimmed.starts_with('[') || trimmed.starts_with('{'))
        && let Ok(val) = serde_json::from_str::<Value>(trimmed)
    {
        let collapsed = collapse_json_arrays(&val, 2);
        if let Ok(pretty) = serde_json::to_string_pretty(&collapsed) {
            return CondensedOutput {
                condensed_bytes: pretty.len(),
                text: pretty,
                raw_bytes,
            };
        }
    }

    let cmd_lower = command.to_ascii_lowercase();
    let semantic = if cmd_lower.contains("git diff") {
        condense_git_diff(&clean)
    } else if cmd_lower.contains("git log") {
        condense_git_log(&clean)
    } else if cmd_lower.contains("git status") {
        condense_git_status(&clean)
    } else if cmd_lower.starts_with("ls ") || cmd_lower == "ls" {
        condense_ls(&clean)
    } else if cmd_lower.contains("cargo ")
        || cmd_lower.contains("pytest")
        || cmd_lower.contains("jest")
        || cmd_lower.contains("vitest")
        || cmd_lower.contains("tsc")
        || cmd_lower.contains("eslint")
    {
        condense_build_or_test(&clean)
    } else if cmd_lower.contains("docker") || cmd_lower.contains("kubectl") {
        condense_container_logs(&clean)
    } else {
        clean
    };

    let deduped = collapse_repeated_lines(&semantic);
    let condensed_bytes = deduped.len();
    CondensedOutput {
        text: deduped,
        raw_bytes,
        condensed_bytes,
    }
}

pub fn simulate_carriage_returns(input: &str) -> String {
    let mut out_lines = Vec::new();
    for raw_line in input.split('\n') {
        let stripped = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if stripped.contains('\r') {
            // Retain the last non-empty frame overwritten by progress-bar `\r`.
            let kept = stripped
                .rsplit('\r')
                .find(|seg| !seg.is_empty())
                .unwrap_or("");
            out_lines.push(kept);
        } else {
            out_lines.push(stripped);
        }
    }
    out_lines.join("\n")
}

pub fn strip_ansi(input: &str) -> String {
    let bytes = strip_ansi_escapes::strip(input.as_bytes());
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn collapse_repeated_lines(input: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut prev: Option<&str> = None;
    let mut run_count = 0usize;

    let flush = |prev_line: Option<&str>, count: usize, dst: &mut Vec<String>| {
        if let Some(line) = prev_line {
            if count > 1 {
                dst.push(format!("{line} [repeated ×{count}]"));
            } else {
                dst.push(line.to_string());
            }
        }
    };

    for line in input.lines() {
        if prev == Some(line) {
            run_count += 1;
        } else {
            flush(prev, run_count, &mut out);
            prev = Some(line);
            run_count = 1;
        }
    }
    flush(prev, run_count, &mut out);
    out.join("\n")
}

fn condense_build_or_test(input: &str) -> String {
    let mut kept = Vec::new();
    let mut omitted_ok = 0usize;
    let mut in_error_block = 0usize;

    for line in input.lines() {
        let trim = line.trim();
        let is_important = trim.contains("error")
            || trim.contains("ERROR")
            || trim.contains("FAIL")
            || trim.contains("FAILED")
            || trim.contains("panicked")
            || trim.contains("warning[")
            || trim.starts_with("test result:")
            || trim.starts_with("Summary ")
            || trim.starts_with("Failures:")
            || trim.starts_with("failures:")
            || trim.starts_with("---- ")
            || trim.contains("assert")
            || trim.contains("TS") && trim.contains(": error");

        if is_important {
            if omitted_ok > 0 {
                kept.push(format!("... [{omitted_ok} passing/build lines omitted]"));
                omitted_ok = 0;
            }
            kept.push(line.to_string());
            in_error_block = 6;
            continue;
        }

        if in_error_block > 0 {
            kept.push(line.to_string());
            in_error_block -= 1;
            continue;
        }

        let is_noisy = trim.starts_with("Compiling ")
            || trim.starts_with("Downloading ")
            || trim.starts_with("Downloaded ")
            || trim.starts_with("Checking ")
            || trim.starts_with("Fresh ")
            || trim.starts_with("Locking ")
            || (trim.starts_with("test ") && trim.ends_with("... ok"))
            || (trim.starts_with("PASS "))
            || trim.ends_with(" PASSED");

        if is_noisy {
            omitted_ok += 1;
        } else if !trim.is_empty() {
            if omitted_ok > 0 {
                kept.push(format!("... [{omitted_ok} passing/build lines omitted]"));
                omitted_ok = 0;
            }
            kept.push(line.to_string());
        }
    }
    if omitted_ok > 0 {
        kept.push(format!("... [{omitted_ok} passing/build lines omitted]"));
    }
    kept.join("\n")
}

fn condense_git_diff(input: &str) -> String {
    let mut out = Vec::new();
    let mut skip_lockfile = false;
    for line in input.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            skip_lockfile = rest.contains("Cargo.lock")
                || rest.contains("package-lock.json")
                || rest.contains("yarn.lock")
                || rest.contains("pnpm-lock.yaml");
            if skip_lockfile {
                out.push(format!("diff --git {rest} [lockfile diff omitted]"));
                continue;
            }
        }
        if !skip_lockfile {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn condense_git_log(input: &str) -> String {
    static COMMIT_RE: OnceLock<Regex> = OnceLock::new();
    let re = COMMIT_RE
        .get_or_init(|| Regex::new(r"^commit ([0-9a-f]{7,40})").unwrap_or_else(|_| unreachable!()));

    let mut out = Vec::new();
    let mut current_hash: Option<String> = None;
    for line in input.lines() {
        if let Some(cap) = re.captures(line) {
            if let Some(h) = cap.get(1) {
                current_hash = Some(h.as_str().chars().take(7).collect());
            }
            continue;
        }
        if line.starts_with("Author:") || line.starts_with("Date:") || line.trim().is_empty() {
            continue;
        }
        if let Some(hash) = current_hash.take() {
            out.push(format!("{hash} {}", line.trim()));
        } else {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

fn condense_git_status(input: &str) -> String {
    let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut other = Vec::new();
    for line in input.lines() {
        if line.len() >= 4
            && line.as_bytes().get(2) == Some(&b' ')
            && !line.starts_with("On ")
            && !line.starts_with("  ")
        {
            let status = line[..2].trim();
            let path = line[3..].trim();
            let (dir, file) = match path.rsplit_once('/') {
                Some((d, f)) => (d.to_string(), f.to_string()),
                None => (".".to_string(), path.to_string()),
            };
            by_dir
                .entry(dir)
                .or_default()
                .push(format!("{status}:{file}"));
        } else if !line.trim().is_empty() {
            other.push(line.to_string());
        }
    }
    if by_dir.is_empty() {
        return input.to_string();
    }
    for (dir, files) in by_dir {
        other.push(format!("{dir}/ ({}): {}", files.len(), files.join(", ")));
    }
    other.join("\n")
}

fn condense_ls(input: &str) -> String {
    let mut entries = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("total ") {
            continue;
        }
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() >= 9 {
            let perms = cols[0];
            let size = cols[4];
            let name = cols[8..].join(" ");
            let mark = if perms.starts_with('d') { "/" } else { "" };
            entries.push(format!("{name}{mark} ({size}B)"));
        } else {
            entries.push(trimmed.to_string());
        }
    }
    entries.join("\n")
}

fn condense_container_logs(input: &str) -> String {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for line in input.lines() {
        let l = line.to_ascii_lowercase();
        if l.contains("error")
            || l.contains("warn")
            || l.contains("fail")
            || l.contains("crash")
            || l.contains("oom")
            || l.contains("exception")
        {
            if skipped > 0 {
                out.push(format!("... [{skipped} info/debug lines omitted]"));
                skipped = 0;
            }
            out.push(line.to_string());
        } else if l.contains("debug") || l.contains("healthz") || l.contains("readyz") {
            skipped += 1;
        } else {
            out.push(line.to_string());
        }
    }
    if skipped > 0 {
        out.push(format!("... [{skipped} info/debug lines omitted]"));
    }
    out.join("\n")
}

pub fn collapse_json_arrays(val: &Value, sample_items: usize) -> Value {
    match val {
        Value::Array(arr) => {
            if arr.len() > sample_items + 1 {
                let mut out: Vec<Value> = arr
                    .iter()
                    .take(sample_items)
                    .map(|item| collapse_json_arrays(item, sample_items))
                    .collect();
                out.push(Value::String(format!(
                    "...[{} more items omitted]",
                    arr.len() - sample_items
                )));
                Value::Array(out)
            } else {
                Value::Array(
                    arr.iter()
                        .map(|item| collapse_json_arrays(item, sample_items))
                        .collect(),
                )
            }
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), collapse_json_arrays(v, sample_items));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn condenses_large_cargo_test_log_and_preserves_errors() {
        let mut log = String::new();
        for i in 0..2500 {
            log.push_str(&format!("   Compiling crate_{i} v0.1.0\n"));
            log.push_str(&format!("test mod::case_{i} ... ok\n"));
        }
        log.push_str("test mod::broken ... FAILED\n");
        log.push_str("thread 'mod::broken' panicked at src/lib.rs:42: assertion failed\n");
        log.push_str("test result: FAILED. 2500 passed; 1 failed\n");

        let out = condense_cli_output("cargo test", &log);
        assert!(out.savings_ratio() >= 0.90);
        assert!(out.text.contains("mod::broken"));
        assert!(out.text.contains("assertion failed"));
    }

    #[test]
    fn simulates_cr_strips_ansi_and_collapses_json_arrays() {
        let progress =
            "\x1b[32mDownloading 10%\rDownloading 50%\rDownloading 100%\x1b[0m\nDone\nDone\nDone\n";
        let out = condense_cli_output("curl", progress);
        assert!(out.text.contains("Downloading 100%"));
        assert!(!out.text.contains("Downloading 10%"));
        assert!(out.text.contains("Done [repeated ×3]"));

        let items: Vec<_> = (0..50)
            .map(|i| serde_json::json!({"id": i, "name": format!("mục_{i}")}))
            .collect();
        let json_raw = serde_json::to_string(&items).unwrap();
        let json_out = condense_cli_output("api", &json_raw);
        assert!(json_out.text.contains("48 more items omitted"));
        assert!(json_out.text.contains("mục_0"));
    }

    #[test]
    fn condenses_git_diff_lockfile_and_git_log() {
        let diff = "diff --git a/Cargo.lock b/Cargo.lock\n+huge\n+lines\ndiff --git a/src/a.rs b/src/a.rs\n+let x = 1;\n";
        let out = condense_cli_output("git diff", diff);
        assert!(out.text.contains("[lockfile diff omitted]"));
        assert!(out.text.contains("+let x = 1;"));

        let log =
            "commit 1234567890abcdef1234567890\nAuthor: Dev\nDate: Today\n\n    Fix WAL lock\n";
        let log_out = condense_cli_output("git log", log);
        assert_eq!(log_out.text.trim(), "1234567 Fix WAL lock");
    }
}
