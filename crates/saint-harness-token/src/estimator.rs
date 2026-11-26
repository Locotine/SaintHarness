use saint_harness_schema::{ContentPart, Message, ToolResultBlock};
use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

pub const ESTIMATE_ERROR_MULTIPLIER: f64 = 1.15;
pub const WINDOW_HEADROOM: f64 = 0.01;
pub const OUTPUT_TOKEN_MIN: u64 = 1_024;
pub const OUTPUT_TOKEN_FALLBACK: u64 = 32_000;
pub const OUTPUT_TOKEN_MAX: u64 = 256_000;

fn bpe() -> Option<&'static CoreBPE> {
    static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().ok()).as_ref()
}

pub fn estimate_text_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    if let Some(enc) = bpe() {
        enc.encode_ordinary(text).len() as u64
    } else {
        ((text.len() as f64) / 3.8).ceil() as u64
    }
}

pub fn estimate_message_tokens(msg: &Message) -> u64 {
    let mut tokens = 4u64; // Role/message framing overhead
    for part in &msg.parts {
        tokens = tokens.saturating_add(estimate_part_tokens(part));
    }
    tokens
}

pub fn estimate_part_tokens(part: &ContentPart) -> u64 {
    match part {
        ContentPart::Text { text, .. } => estimate_text_tokens(text),
        ContentPart::Reasoning { text, .. } => estimate_text_tokens(text),
        ContentPart::Media { base64_data, .. } => {
            // Vision tile proxy: ~1 token per 750 base64 bytes, minimum 256 tokens.
            ((base64_data.len() as u64) / 750).max(256)
        }
        ContentPart::ToolCall {
            name, arguments, ..
        } => {
            let args = serde_json::to_string(arguments).unwrap_or_default();
            estimate_text_tokens(name) + estimate_text_tokens(&args) + 8
        }
        ContentPart::ToolResult { content, .. } => {
            content
                .iter()
                .map(|b| match b {
                    ToolResultBlock::Text { text } => estimate_text_tokens(text),
                    ToolResultBlock::Media { base64_data, .. } => {
                        ((base64_data.len() as u64) / 750).max(256)
                    }
                })
                .sum::<u64>()
                + 6
        }
        ContentPart::Compaction {
            summary_xml: Some(s),
            ..
        } => estimate_text_tokens(s),
        ContentPart::Compaction { .. } => 64,
        ContentPart::EffortMarker { effort, .. } => estimate_text_tokens(effort) + 2,
    }
}

/// Anchor estimator: last provider-measured input tokens + conservative 1.15x delta estimate.
pub fn anchor_estimate_tokens(last_measured_input: u64, delta_messages: &[Message]) -> u64 {
    let delta_raw: u64 = delta_messages.iter().map(estimate_message_tokens).sum();
    let padded = ((delta_raw as f64) * ESTIMATE_ERROR_MULTIPLIER).ceil() as u64;
    last_measured_input.saturating_add(padded)
}

/// Computes `min(requested, max(1024, floor(ctx * 0.99) - measured - ceil(estimated * 1.15)))`.
pub fn compute_output_limit(
    requested: u64,
    context_window: u64,
    measured_input: u64,
    estimated_delta: u64,
) -> u64 {
    let usable = ((context_window as f64) * (1.0 - WINDOW_HEADROOM)).floor() as u64;
    let padded_delta = ((estimated_delta as f64) * ESTIMATE_ERROR_MULTIPLIER).ceil() as u64;
    let remaining = usable
        .saturating_sub(measured_input)
        .saturating_sub(padded_delta)
        .max(OUTPUT_TOKEN_MIN);
    requested.min(OUTPUT_TOKEN_MAX).min(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_limit_formula_clamps_correctly() {
        assert_eq!(compute_output_limit(16_384, 100_000, 90_000, 1_000), 7_850);
        assert_eq!(compute_output_limit(16_384, 100_000, 98_500, 1_000), 1_024);
    }

    #[test]
    fn anchor_estimator_applies_1_15_safety_factor() {
        let est = anchor_estimate_tokens(10_000, &[]);
        assert_eq!(est, 10_000);
        assert!(estimate_text_tokens("fn main() { println!(\"hello\"); }") >= 6);
    }
}
