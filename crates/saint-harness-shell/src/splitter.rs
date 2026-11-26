/// Lightweight shell command splitter and ARITY classifier for permission-rule suggestions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSubcommand {
    pub raw: String,
    pub tokens: Vec<String>,
    pub suggested_pattern: String,
    pub is_dangerous_for_always: bool,
}

const TWO_WORD_ARITY: &[&str] = &[
    "git",
    "cargo",
    "npm",
    "pnpm",
    "yarn",
    "docker",
    "kubectl",
    "systemctl",
    "apt",
    "brew",
    "rustup",
    "python",
    "python3",
    "pip",
    "uv",
    "go",
];

const DANGEROUS_ALWAYS_PREFIXES: &[&str] = &["sudo", "su", "bash", "sh", "zsh", "rm", "dd", "mkfs"];

pub fn split_shell_commands(command: &str) -> Vec<ParsedSubcommand> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            current.push(ch);
            escaped = false;
            i += 1;
            continue;
        }
        if ch == '\\' && !in_single {
            escaped = true;
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            current.push(ch);
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            let is_two = (ch == '&' && chars.get(i + 1) == Some(&'&'))
                || (ch == '|' && chars.get(i + 1) == Some(&'|'));
            if is_two {
                if !current.trim().is_empty() {
                    segments.push(current.trim().to_string());
                }
                current.clear();
                i += 2;
                continue;
            }
            if matches!(ch, ';' | '|' | '\n') {
                if !current.trim().is_empty() {
                    segments.push(current.trim().to_string());
                }
                current.clear();
                i += 1;
                continue;
            }
        }

        current.push(ch);
        i += 1;
    }
    if !current.trim().is_empty() {
        segments.push(current.trim().to_string());
    }

    segments.into_iter().map(classify_segment).collect()
}

fn classify_segment(raw: String) -> ParsedSubcommand {
    let tokens = tokenize_words(&raw);
    let first = tokens.first().map(String::as_str).unwrap_or("");
    let second = tokens.get(1).map(String::as_str);

    let suggested_pattern = if TWO_WORD_ARITY.contains(&first) {
        match second {
            Some(sub) if !sub.starts_with('-') => format!("{first} {sub} *"),
            _ => format!("{first} *"),
        }
    } else if !first.is_empty() {
        format!("{first} *")
    } else {
        raw.clone()
    };

    let is_dangerous_for_always = DANGEROUS_ALWAYS_PREFIXES.contains(&first)
        || (first == "bash" && second == Some("-c"))
        || (first == "sh" && second == Some("-c"));

    ParsedSubcommand {
        raw,
        tokens,
        suggested_pattern,
        is_dangerous_for_always,
    }
}

fn tokenize_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in s.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_pipelines_and_suggests_arity_patterns() {
        let parsed = split_shell_commands("cargo nextest run -p core && sudo rm -rf /tmp/x");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].suggested_pattern, "cargo nextest *");
        assert!(!parsed[0].is_dangerous_for_always);
        assert!(parsed[1].is_dangerous_for_always);
    }
}
