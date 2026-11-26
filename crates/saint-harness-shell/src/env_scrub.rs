use std::collections::BTreeMap;

/// Removes sensitive environment variables (`SAINT_AGENT_GATEWAY_*`, `*_API_KEY`, `*_TOKEN`, `*_SECRET*`)
/// before spawning shell subprocesses unless explicitly permitted in `allowlist`.
pub fn scrubbed_env(
    base: impl IntoIterator<Item = (String, String)>,
    allowlist: &[String],
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (k, v) in base {
        if allowlist.iter().any(|allowed| allowed == &k) || !is_sensitive_env_key(&k) {
            out.insert(k, v);
        }
    }
    out
}

pub fn is_sensitive_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.starts_with("SAINT_AGENT_GATEWAY_")
        || upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.contains("_SECRET")
        || upper == "ANTHROPIC_API_KEY"
        || upper == "GEMINI_API_KEY"
        || upper == "DASHSCOPE_API_KEY"
        || upper == "OPENAI_API_KEY"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_api_keys_tokens_secrets_and_gateway_vars() {
        let env = vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-1".to_string()),
            ("SAINT_AGENT_GATEWAY_TOKEN".to_string(), "tok".to_string()),
            ("MY_DB_SECRET_KEY".to_string(), "sec".to_string()),
            ("ALLOWED_TOKEN".to_string(), "keep".to_string()),
        ];
        let clean = scrubbed_env(env, &["ALLOWED_TOKEN".to_string()]);
        assert_eq!(clean.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(clean.get("ALLOWED_TOKEN").map(String::as_str), Some("keep"));
        assert!(!clean.contains_key("ANTHROPIC_API_KEY"));
        assert!(!clean.contains_key("SAINT_AGENT_GATEWAY_TOKEN"));
        assert!(!clean.contains_key("MY_DB_SECRET_KEY"));
    }
}
