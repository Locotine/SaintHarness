use crate::estimator::estimate_message_tokens;
use saint_harness_schema::{ContentPart, Message, Role, ToolResultBlock};

pub const SHRINK_STEPS: [f64; 4] = [0.85, 0.70, 0.50, 0.35];

#[derive(Debug, Clone)]
pub struct ReducedHistory {
    pub messages: Vec<Message>,
    pub level_used: u8,
    pub estimated_tokens: u64,
}

/// 5-level reducer (levels 0..=4) guaranteeing convergence under `target_tokens` while
/// never dropping the final user/assistant exchange.
pub fn flatten_and_drop_oldest(messages: &[Message], target_tokens: u64) -> ReducedHistory {
    for level in 0u8..=4 {
        let candidate = reduce_at_level(messages, target_tokens, level);
        let est: u64 = candidate.iter().map(estimate_message_tokens).sum();
        if est <= target_tokens || level == 4 {
            return ReducedHistory {
                messages: candidate,
                level_used: level,
                estimated_tokens: est,
            };
        }
    }
    ReducedHistory {
        messages: messages.to_vec(),
        level_used: 4,
        estimated_tokens: messages.iter().map(estimate_message_tokens).sum(),
    }
}

fn reduce_at_level(messages: &[Message], target_tokens: u64, level: u8) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut work: Vec<Message> = messages
        .iter()
        .map(|m| match level {
            0 => m.clone(),
            1 => strip_reasoning_and_media(m),
            2 => truncate_tool_results(m, 512),
            3 => flatten_message_to_text(m, 400),
            _ => flatten_message_to_text(m, 160),
        })
        .collect();

    // Preserve at least the final exchange (up to last 2 messages) in O(N) time.
    let keep_min = work.len().min(2);
    let msg_tokens: Vec<u64> = work.iter().map(estimate_message_tokens).collect();
    let mut total: u64 = msg_tokens.iter().sum();
    let mut drop_count = 0usize;
    while work.len().saturating_sub(drop_count) > keep_min && total > target_tokens {
        total = total.saturating_sub(msg_tokens[drop_count]);
        drop_count += 1;
    }
    if drop_count > 0 {
        work.drain(..drop_count);
    }

    if level == 4 && total > target_tokens {
        // Hard-cap final exchange if even the last 2 messages exceed target_tokens.
        let max_chars = ((target_tokens as usize).saturating_mul(2) / work.len().max(1)).max(120);
        for m in &mut work {
            *m = flatten_message_to_text(m, max_chars);
        }
    }

    work
}

fn strip_reasoning_and_media(msg: &Message) -> Message {
    let mut out = msg.clone();
    out.parts = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Reasoning { .. } => None,
            ContentPart::Media { source_path, .. } => Some(ContentPart::Text {
                text: format!(
                    "[image omitted: {}]",
                    source_path.as_deref().unwrap_or("inline")
                ),
                provider_meta: None,
            }),
            other => Some(other.clone()),
        })
        .collect();
    out
}

fn truncate_tool_results(msg: &Message, max_chars: usize) -> Message {
    let mut out = strip_reasoning_and_media(msg);
    out.parts = out
        .parts
        .into_iter()
        .map(|p| match p {
            ContentPart::ToolResult {
                call_id,
                name,
                content,
                is_error,
            } => {
                let text = content
                    .iter()
                    .filter_map(|b| match b {
                        ToolResultBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let clipped: String = text.chars().take(max_chars).collect();
                ContentPart::ToolResult {
                    call_id,
                    name,
                    content: vec![ToolResultBlock::Text { text: clipped }],
                    is_error,
                }
            }
            other => other,
        })
        .collect();
    out
}

fn flatten_message_to_text(msg: &Message, max_chars: usize) -> Message {
    let mut buf = String::new();
    for p in &msg.parts {
        match p {
            ContentPart::Text { text, .. } => {
                buf.push_str(text);
                buf.push('\n');
            }
            ContentPart::ToolCall { name, .. } => {
                buf.push_str(&format!("[tool_call:{name}]\n"));
            }
            ContentPart::ToolResult { name, content, .. } => {
                let first = content
                    .iter()
                    .find_map(|b| match b {
                        ToolResultBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or("");
                buf.push_str(&format!("[tool_result:{name}] {first}\n"));
            }
            ContentPart::Compaction {
                summary_xml: Some(s),
                ..
            } => {
                buf.push_str(s);
                buf.push('\n');
            }
            _ => {}
        }
    }
    let clipped: String = buf.chars().take(max_chars).collect();
    Message {
        id: msg.id.clone(),
        parent_msg_id: msg.parent_msg_id.clone(),
        role: if msg.role == Role::Tool {
            Role::User
        } else {
            msg.role
        },
        parts: vec![ContentPart::Text {
            text: clipped,
            provider_meta: None,
        }],
        model_provider: msg.model_provider.clone(),
        model_id: msg.model_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reducer_converges_on_300k_token_history_and_keeps_last_exchange() {
        let mut msgs = Vec::new();
        for i in 0..300 {
            msgs.push(Message {
                id: format!("m_{i}"),
                parent_msg_id: None,
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                parts: vec![ContentPart::Text {
                    text: format!("turn {i} ").repeat(600),
                    provider_meta: None,
                }],
                model_provider: None,
                model_id: None,
            });
        }
        let reduced = flatten_and_drop_oldest(&msgs, 8_000);
        assert!(reduced.estimated_tokens <= 8_000);
        assert!(!reduced.messages.is_empty());
        assert_eq!(
            reduced.messages.last().map(|m| m.id.as_str()),
            Some("m_299")
        );
    }
}
