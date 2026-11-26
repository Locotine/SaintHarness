use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::OnceLock;
use ulid::Ulid;

#[derive(Debug, Clone, PartialEq)]
pub struct RescuedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default)]
pub struct ThinkSplitState {
    in_think: bool,
    pending: String,
    pub captured_think: String,
}

impl ThinkSplitState {
    /// Incrementally splits `<think>...</think>` blocks embedded inside `delta.content`.
    /// Returns `(visible_text_delta, reasoning_delta)`.
    pub fn push(&mut self, chunk: &str) -> (String, String) {
        self.pending.push_str(chunk);
        let mut visible = String::new();
        let mut reasoning = String::new();

        loop {
            if self.in_think {
                if let Some(pos) = self.pending.find("</think>") {
                    let body = &self.pending[..pos];
                    reasoning.push_str(body);
                    self.captured_think.push_str(body);
                    self.pending.drain(..pos + "</think>".len());
                    self.in_think = false;
                } else {
                    let keep = partial_tag_suffix_len(&self.pending, "</think>");
                    let emit_len = self.pending.len().saturating_sub(keep);
                    if emit_len > 0 {
                        let body = &self.pending[..emit_len];
                        reasoning.push_str(body);
                        self.captured_think.push_str(body);
                        self.pending.drain(..emit_len);
                    }
                    break;
                }
            } else if let Some(pos) = self.pending.find("<think>") {
                visible.push_str(&self.pending[..pos]);
                self.pending.drain(..pos + "<think>".len());
                self.in_think = true;
            } else {
                let keep = partial_tag_suffix_len(&self.pending, "<think>");
                let emit_len = self.pending.len().saturating_sub(keep);
                if emit_len > 0 {
                    visible.push_str(&self.pending[..emit_len]);
                    self.pending.drain(..emit_len);
                }
                break;
            }
        }
        (visible, reasoning)
    }

    pub fn flush(&mut self) -> (String, String) {
        let rest = std::mem::take(&mut self.pending);
        if self.in_think {
            self.captured_think.push_str(&rest);
            (String::new(), rest)
        } else {
            (rest, String::new())
        }
    }
}

fn partial_tag_suffix_len(s: &str, tag: &str) -> usize {
    let max_check = s.len().min(tag.len().saturating_sub(1));
    for len in (1..=max_check).rev() {
        if s.is_char_boundary(s.len() - len) && tag.starts_with(&s[s.len() - len..]) {
            return len;
        }
    }
    0
}

/// Repairs common Qwen/local-model JSON quirks: markdown code fences and trailing commas.
pub fn repair_json_arguments(raw: &str) -> Result<Value, serde_json::Error> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return Ok(v);
    }
    let stripped = strip_code_fences(raw);
    if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
        return Ok(v);
    }
    let no_trailing = remove_trailing_commas(&stripped);
    serde_json::from_str::<Value>(&no_trailing)
}

fn strip_code_fences(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let after_lang = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
        let cleaned = after_lang
            .trim_end()
            .strip_suffix("```")
            .unwrap_or(after_lang);
        cleaned.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn remove_trailing_commas(input: &str) -> String {
    static TRAILING_COMMA_RE: OnceLock<Regex> = OnceLock::new();
    let re = TRAILING_COMMA_RE
        .get_or_init(|| Regex::new(r",(\s*[}\]])").unwrap_or_else(|_| unreachable!()));
    re.replace_all(input, "$1").into_owned()
}

/// Thinking-only guard: rescues `<tool_call>{...}</tool_call>` or XML `<function=name><parameter=k>v</parameter></function>`
/// trapped inside reasoning or raw assistant content.
pub fn rescue_embedded_tool_calls(
    text: &str,
    seen_ids: &mut HashSet<String>,
) -> Vec<RescuedToolCall> {
    let mut rescued = Vec::new();

    static TOOL_CALL_TAG_RE: OnceLock<Regex> = OnceLock::new();
    let tag_re = TOOL_CALL_TAG_RE.get_or_init(|| {
        Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap_or_else(|_| unreachable!())
    });

    for cap in tag_re.captures_iter(text) {
        if let Some(body) = cap.get(1).map(|m| m.as_str().trim()) {
            if let Ok(val) = repair_json_arguments(body)
                && let Some(name) = val.get("name").and_then(|v| v.as_str())
            {
                let args = val
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                let id =
                    ensure_unique_tool_call_id(val.get("id").and_then(|v| v.as_str()), seen_ids);
                rescued.push(RescuedToolCall {
                    id,
                    name: name.to_string(),
                    arguments: args,
                });
                continue;
            }
            for xml_call in parse_xml_function_calls(body, seen_ids) {
                rescued.push(xml_call);
            }
        }
    }

    if rescued.is_empty() {
        rescued.extend(parse_xml_function_calls(text, seen_ids));
    }

    rescued
}

pub fn parse_xml_function_calls(
    text: &str,
    seen_ids: &mut HashSet<String>,
) -> Vec<RescuedToolCall> {
    static FN_RE: OnceLock<Regex> = OnceLock::new();
    static PARAM_RE: OnceLock<Regex> = OnceLock::new();
    let fn_re = FN_RE.get_or_init(|| {
        Regex::new(r"(?s)<function=([A-Za-z0-9_.:-]+)>(.*?)</function>")
            .unwrap_or_else(|_| unreachable!())
    });
    let param_re = PARAM_RE.get_or_init(|| {
        Regex::new(r"(?s)<parameter=([A-Za-z0-9_.-]+)>(.*?)</parameter>")
            .unwrap_or_else(|_| unreachable!())
    });

    let mut out = Vec::new();
    for cap in fn_re.captures_iter(text) {
        let Some(name) = cap.get(1).map(|m| m.as_str().trim()) else {
            continue;
        };
        let body = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        let mut map = Map::new();
        for pcap in param_re.captures_iter(body) {
            if let (Some(k), Some(v)) = (pcap.get(1), pcap.get(2)) {
                let raw_val = v.as_str().trim();
                let parsed = serde_json::from_str::<Value>(raw_val)
                    .unwrap_or_else(|_| Value::String(raw_val.to_string()));
                map.insert(k.as_str().to_string(), parsed);
            }
        }
        let id = ensure_unique_tool_call_id(None, seen_ids);
        out.push(RescuedToolCall {
            id,
            name: name.to_string(),
            arguments: Value::Object(map),
        });
    }
    out
}

pub fn ensure_unique_tool_call_id(raw: Option<&str>, seen: &mut HashSet<String>) -> String {
    if let Some(candidate) = raw.map(str::trim).filter(|s| !s.is_empty())
        && seen.insert(candidate.to_string())
    {
        return candidate.to_string();
    }
    loop {
        let id = format!("call_{}", Ulid::new());
        if seen.insert(id.clone()) {
            return id;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_think_tags_across_arbitrary_chunks() {
        let mut st = ThinkSplitState::default();
        let mut vis = String::new();
        let mut reas = String::new();
        for chunk in ["Hel", "lo <thi", "nk>plan tiếng Việt</th", "ink> world"] {
            let (v, r) = st.push(chunk);
            vis.push_str(&v);
            reas.push_str(&r);
        }
        let (v, r) = st.flush();
        vis.push_str(&v);
        reas.push_str(&r);
        assert_eq!(vis, "Hello  world");
        assert_eq!(reas, "plan tiếng Việt");
    }

    #[test]
    fn rescues_json_tool_call_and_xml_function_call() {
        let mut seen = HashSet::new();
        let trapped = r#"<tool_call>```json
{"name": "read", "arguments": {"path": "Cargo.toml",}}
```</tool_call>"#;
        let rescued = rescue_embedded_tool_calls(trapped, &mut seen);
        assert_eq!(rescued.len(), 1);
        assert_eq!(rescued[0].name, "read");
        assert_eq!(rescued[0].arguments, json!({"path": "Cargo.toml"}));

        let xml = r#"<function=edit><parameter=path>src/lib.rs</parameter><parameter=line>42</parameter></function>"#;
        let rescued_xml = rescue_embedded_tool_calls(xml, &mut seen);
        assert_eq!(rescued_xml.len(), 1);
        assert_eq!(rescued_xml[0].name, "edit");
        assert_eq!(
            rescued_xml[0].arguments,
            json!({"path": "src/lib.rs", "line": 42})
        );
    }

    #[test]
    fn deduplicates_empty_and_repeated_tool_call_ids() {
        let mut seen = HashSet::new();
        let a = ensure_unique_tool_call_id(Some("call_1"), &mut seen);
        let b = ensure_unique_tool_call_id(Some("call_1"), &mut seen);
        let c = ensure_unique_tool_call_id(Some(""), &mut seen);
        assert_eq!(a, "call_1");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(b.starts_with("call_"));
    }
}
