use crate::estimator::{estimate_message_tokens, estimate_part_tokens};
use crate::truncation::write_zstd_spill;
use saint_harness_schema::{ContentPart, Message, ToolResultBlock};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct ShakeOutcome {
    pub messages: Vec<Message>,
    pub before_tokens: u64,
    pub after_tokens: u64,
    pub spilled_blocks: usize,
    pub below_target: bool,
}

/// Tier-1 zero-LLM compaction: spills bulky older blocks to `local://spill/<id>.zst`
/// until estimated context drops below 55% of `ceiling_tokens`.
pub fn run_tier1_shake(
    messages: &[Message],
    ceiling_tokens: u64,
    keep_recent_messages: usize,
    spill_dir: Option<&Path>,
) -> ShakeOutcome {
    let target_tokens = ((ceiling_tokens as f64) * 0.55).floor() as u64;
    let before_tokens: u64 = messages.iter().map(estimate_message_tokens).sum();
    if before_tokens < target_tokens {
        return ShakeOutcome {
            messages: messages.to_vec(),
            before_tokens,
            after_tokens: before_tokens,
            spilled_blocks: 0,
            below_target: true,
        };
    }

    let mut out = messages.to_vec();
    let mut current_tokens = before_tokens;
    let mut spilled_blocks = 0usize;
    let mutable_end = out.len().saturating_sub(keep_recent_messages);

    'outer: for msg in &mut out[..mutable_end] {
        for part in &mut msg.parts {
            if current_tokens < target_tokens {
                break 'outer;
            }
            if let ContentPart::ToolResult {
                call_id,
                name,
                content,
                ..
            } = part
            {
                let part_tokens = estimate_part_tokens(&ContentPart::ToolResult {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    content: content.clone(),
                    is_error: false,
                });
                if part_tokens < 250 {
                    continue;
                }
                let raw_joined = content
                    .iter()
                    .filter_map(|b| match b {
                        ToolResultBlock::Text { text } => Some(text.as_str()),
                        ToolResultBlock::Media { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let spill_id = format!("shake-{call_id}");
                if let Some(dir) = spill_dir {
                    let _ = write_zstd_spill(dir, &spill_id, raw_joined.as_bytes());
                }
                let replacement = format!(
                    "[spilled {name} (~{part_tokens} tok) -> local://spill/{spill_id}.out.zst]"
                );
                let new_tokens = (replacement.len() as u64 / 4).max(16);
                *content = vec![ToolResultBlock::Text { text: replacement }];
                current_tokens = current_tokens
                    .saturating_sub(part_tokens)
                    .saturating_add(new_tokens);
                spilled_blocks += 1;
            }
        }
    }

    ShakeOutcome {
        messages: out,
        before_tokens,
        after_tokens: current_tokens,
        spilled_blocks,
        below_target: current_tokens < target_tokens,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::Role;

    #[test]
    fn shake_drops_context_below_55_percent_without_llm() {
        let tmp = tempfile::tempdir().unwrap();
        let mut msgs = Vec::new();
        for i in 0..10 {
            msgs.push(Message {
                id: format!("m_{i}"),
                parent_msg_id: None,
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    call_id: format!("call_{i}"),
                    name: "shell".into(),
                    content: vec![ToolResultBlock::Text {
                        text: "compiler output line with details\n".repeat(400),
                    }],
                    is_error: false,
                }],
                model_provider: None,
                model_id: None,
            });
        }
        let outcome = run_tier1_shake(&msgs, 20_000, 2, Some(tmp.path()));
        assert!(outcome.below_target);
        assert!(outcome.spilled_blocks > 0);
        assert!(outcome.after_tokens < 11_000);
    }
}
