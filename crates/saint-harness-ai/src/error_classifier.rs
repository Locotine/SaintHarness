use regex::Regex;
use saint_harness_schema::{AiError, TransportOp};
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const RETRY_AFTER_CAP_MS: u64 = 15 * 60 * 1000;

fn overflow_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw = [
            r"(?i)prompt is too long",
            r"(?i)context_length_exceeded",
            r"(?i)maximum context length",
            r"(?i)exceeds? the (model'?s )?(maximum )?(context|token) (length|limit|window)",
            r"(?i)input is too long",
            r"(?i)too many (input )?tokens",
            r"(?i)token limit exceeded",
            r"(?i)context window (exceeded|overflow|limit)",
            r"(?i)max_tokens.*exceed",
            r"(?i)input token count.*exceeds",
            r"(?i)request.*exceeds.*context",
            r"(?i)reduce the length of the messages",
            r"(?i)please reduce (the length|your prompt)",
            r"(?i)prompt_tokens.*>.*max",
            r"(?i)input_tokens.*>.*context",
            r"(?i)max_prompt_tokens",
            r"(?i)context_window_exceeded",
            r"(?i)content_length_exceeded",
            r"(?i)string too long.*context",
            r"(?i)request size exceeds.*token",
            r"(?i)total tokens.*greater than",
            r"(?i)exceeds? max_model_len",
            r"(?i)decoder_input_ids.*exceed",
            r"(?i)kv cache.*full.*prompt",
            r"(?i)maximum number of tokens",
            r"(?i)token count.*exceeds the maximum",
            r"(?i)prompt length.*greater than",
            r"(?i)exceeded model token limit",
            r"(?i)sequence length.*exceeds",
            r"(?i)context size.*exceeded",
            r"(?i)exceeds maximum input length",
            r"(?i)messages.*too long",
            r"(?i)conversation is too long",
            r"(?i)history exceeds.*limit",
            r"(?i)tokens requested.*exceed",
            r"(?i)input length.*above maximum",
            r"(?i)context_overflow",
            r"(?i)exceeds context capacity",
            r"(?i)too long for model",
            r"(?i)max input tokens",
        ];
        raw.iter().filter_map(|p| Regex::new(p).ok()).collect()
    })
}

fn throttle_exclusions() -> &'static [Regex] {
    static EXCL: OnceLock<Vec<Regex>> = OnceLock::new();
    EXCL.get_or_init(|| {
        let raw = [
            r"(?i)tokens? per minute",
            r"(?i)\btpm\b",
            r"(?i)\brpm\b",
            r"(?i)requests? per minute",
            r"(?i)rate[ _-]?limit",
            r"(?i)throttl(ed|ing)",
            r"(?i)too many requests",
        ];
        raw.iter().filter_map(|p| Regex::new(p).ok()).collect()
    })
}

pub fn parse_retry_after_ms(headers: &BTreeMap<String, String>) -> Option<u64> {
    let val = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after-ms"))
        .map(|(_, v)| v.as_str());
    if let Some(ms_str) = val
        && let Ok(ms) = ms_str.trim().parse::<u64>()
    {
        return Some(ms.min(RETRY_AFTER_CAP_MS));
    }

    let sec_val = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
        .map(|(_, v)| v.as_str())?;
    if let Ok(secs) = sec_val.trim().parse::<f64>()
        && secs.is_finite()
        && secs >= 0.0
    {
        return Some(((secs * 1000.0).round() as u64).min(RETRY_AFTER_CAP_MS));
    }
    None
}

pub fn parse_should_retry(headers: &BTreeMap<String, String>) -> Option<bool> {
    let val = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-should-retry"))
        .map(|(_, v)| v.trim())?;
    if val.eq_ignore_ascii_case("true") {
        Some(true)
    } else if val.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

pub fn is_context_overflow_message(body: &str) -> bool {
    if throttle_exclusions().iter().any(|re| re.is_match(body)) {
        return false;
    }
    overflow_patterns().iter().any(|re| re.is_match(body))
}

/// 10-priority HTTP error classifier across OpenAI-compat, Anthropic, and Gemini providers.
pub fn classify_http_error(status: u16, headers: &BTreeMap<String, String>, body: &str) -> AiError {
    let retry_after_ms = parse_retry_after_ms(headers);
    let should_retry_header = parse_should_retry(headers);
    let lower = body.to_ascii_lowercase();
    let msg = extract_error_message(body);

    // 1. Auth failures
    if status == 401 || status == 403 || lower.contains("invalid_api_key") {
        return AiError::Authentication { message: msg };
    }

    // 2. Payload too large (HTTP 413)
    if status == 413 || lower.contains("request_too_large") || lower.contains("entity too large") {
        return AiError::PayloadTooLarge { message: msg };
    }

    // 3. Context overflow (excluding TPM/RPM rate-limit false positives)
    if is_context_overflow_message(body) {
        return AiError::ContextOverflow {
            message: msg,
            raw_body: body.to_string(),
        };
    }

    // 4. Quota / billing exhaustion
    if lower.contains("insufficient_quota")
        || lower.contains("billing")
        || lower.contains("credit balance is too low")
        || lower.contains("quota exceeded")
    {
        return AiError::QuotaExceeded { message: msg };
    }

    // 5. Rate limit (HTTP 429 or explicit TPM/RPM throttle)
    if status == 429 || throttle_exclusions().iter().any(|re| re.is_match(body)) {
        return AiError::RateLimit {
            message: msg,
            retry_after_ms,
            should_retry_header,
        };
    }

    // 6. Content policy / refusal
    if lower.contains("content_policy")
        || lower.contains("safety")
        || lower.contains("refusal")
        || lower.contains("prohibited_content")
    {
        return AiError::ContentPolicy { message: msg };
    }

    // 7. Request timeout
    if status == 408 || status == 504 || lower.contains("timed out") || lower.contains("timeout") {
        return AiError::Timeout { message: msg };
    }

    // 8. Provider 5xx / overload (529)
    if status >= 500 || status == 529 || lower.contains("overloaded_error") {
        return AiError::ProviderInternal {
            status: Some(status),
            message: msg,
            retry_after_ms,
            should_retry_header,
        };
    }

    // 9. Client invalid request (400/404/422)
    if (400..500).contains(&status) {
        return AiError::InvalidRequest { message: msg };
    }

    // 10. Unknown provider error
    AiError::UnknownProvider {
        message: msg,
        raw_body: body.to_string(),
    }
}

pub fn classify_transport_error(message: &str, operation: TransportOp, accepted: bool) -> AiError {
    let lower = message.to_ascii_lowercase();
    let is_refused = lower.contains("connection refused") || lower.contains("econnrefused");
    let network_down = !is_refused
        && (lower.contains("dns")
            || lower.contains("failed to lookup")
            || lower.contains("network is unreachable")
            || lower.contains("host is unreachable")
            || lower.contains("name or service not known"));
    if lower.contains("timed out") || lower.contains("timeout") {
        return AiError::Timeout {
            message: message.to_string(),
        };
    }
    AiError::Transport {
        operation,
        accepted,
        network_down,
        message: message.to_string(),
    }
}

fn extract_error_message(body: &str) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(m) = val
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|s| s.as_str())
        {
            return m.to_string();
        }
        if let Some(m) = val.get("message").and_then(|s| s.as_str()) {
            return m.to_string();
        }
    }
    body.chars().take(240).collect()
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn classifies_all_ten_priority_tiers() {
        let mut h = BTreeMap::new();
        assert!(matches!(
            classify_http_error(401, &h, r#"{"error":{"message":"bad key"}}"#),
            AiError::Authentication { .. }
        ));
        assert!(matches!(
            classify_http_error(413, &h, "payload too large"),
            AiError::PayloadTooLarge { .. }
        ));
        assert!(matches!(
            classify_http_error(
                400,
                &h,
                r#"{"error":{"message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#
            ),
            AiError::ContextOverflow { .. }
        ));
        // TPM/RPM throttle containing "tokens" must NOT be misclassified as ContextOverflow
        assert!(matches!(
            classify_http_error(
                429,
                &h,
                r#"{"error":{"message":"exceeded tokens per minute limit (TPM)"}}"#
            ),
            AiError::RateLimit { .. }
        ));
        assert!(matches!(
            classify_http_error(
                429,
                &h,
                r#"{"error":{"message":"credit balance is too low"}}"#
            ),
            AiError::QuotaExceeded { .. }
        ));
        assert!(matches!(
            classify_http_error(
                400,
                &h,
                r#"{"error":{"message":"content_policy violation"}}"#
            ),
            AiError::ContentPolicy { .. }
        ));
        assert!(matches!(
            classify_http_error(504, &h, "upstream gateway timeout"),
            AiError::Timeout { .. }
        ));
        h.insert("retry-after".to_string(), "99999".to_string());
        h.insert("x-should-retry".to_string(), "true".to_string());
        match classify_http_error(529, &h, "overloaded_error") {
            AiError::ProviderInternal {
                retry_after_ms,
                should_retry_header,
                ..
            } => {
                assert_eq!(retry_after_ms, Some(RETRY_AFTER_CAP_MS));
                assert_eq!(should_retry_header, Some(true));
            }
            other => panic!("expected ProviderInternal, got {other:?}"),
        }
        assert!(matches!(
            classify_http_error(400, &BTreeMap::new(), "missing field"),
            AiError::InvalidRequest { .. }
        ));
        assert!(matches!(
            classify_http_error(200, &BTreeMap::new(), "weird body"),
            AiError::UnknownProvider { .. }
        ));
    }

    #[test]
    fn econnrefused_is_not_network_down() {
        let err =
            classify_transport_error("tcp connect: ECONNREFUSED", TransportOp::Connect, false);
        assert!(matches!(
            err,
            AiError::Transport {
                network_down: false,
                ..
            }
        ));
        let dns =
            classify_transport_error("dns error: failed to lookup host", TransportOp::Dns, false);
        assert!(matches!(
            dns,
            AiError::Transport {
                network_down: true,
                ..
            }
        ));
    }
}
