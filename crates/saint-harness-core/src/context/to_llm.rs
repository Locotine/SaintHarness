use saint_harness_schema::{ContentPart, Message, Role};
use std::collections::HashSet;

pub const GEMINI_UNSIGNED_SENTINEL: &str = "skip_thought_signature_validator";

/// Transforms stored DAG messages into provider-canonical messages enforcing all 4 Cross-Model
/// Handoff invariants (`Qwen <-> Claude <-> Gemini`) and trimming failed assistant turns after
/// the last settled local tool call.
pub fn to_llm_messages(
    history: &[Message],
    target_provider: &str,
    target_model_id: &str,
) -> Vec<Message> {
    let mut out = Vec::with_capacity(history.len());
    for msg in history {
        let same_model = msg.model_provider.as_deref() == Some(target_provider)
            && msg.model_id.as_deref() == Some(target_model_id);
        let transformed = match target_provider {
            "anthropic" => transform_for_claude(msg, same_model),
            "google" => transform_for_gemini(msg, same_model),
            _ => transform_for_qwen(msg),
        };
        if !transformed.parts.is_empty() {
            out.push(transformed);
        }
    }
    trim_incomplete_trailing_assistant(&mut out);
    out
}

fn transform_for_claude(msg: &Message, same_model: bool) -> Message {
    let mut cloned = msg.clone();
    cloned.parts = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Reasoning {
                text,
                signature,
                redacted_data,
                ..
            } => {
                // Invariant #2: replay thinking + signature / redacted_thinking ONLY when same_model.
                if same_model && (signature.is_some() || redacted_data.is_some()) {
                    Some(ContentPart::Reasoning {
                        text: text.clone(),
                        signature: signature.clone(),
                        redacted_data: redacted_data.clone(),
                        provider_meta: None,
                    })
                } else {
                    None
                }
            }
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                provider_executed,
                ..
            } => Some(ContentPart::ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                provider_executed: *provider_executed,
                signature: None,
            }),
            other => Some(other.clone()),
        })
        .collect();
    cloned
}

fn transform_for_gemini(msg: &Message, same_model: bool) -> Message {
    let mut cloned = msg.clone();
    let mut attached_sentinel = false;
    cloned.parts = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Reasoning {
                text, signature, ..
            } => {
                if same_model && signature.is_some() {
                    Some(ContentPart::Reasoning {
                        text: text.clone(),
                        signature: signature.clone(),
                        redacted_data: None,
                        provider_meta: None,
                    })
                } else {
                    None
                }
            }
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                provider_executed,
                signature,
            } => {
                // Invariant #1: attach skip_thought_signature_validator on first unsigned historical tool call.
                let sig = if same_model && signature.is_some() {
                    signature.clone()
                } else if !attached_sentinel {
                    attached_sentinel = true;
                    Some(GEMINI_UNSIGNED_SENTINEL.to_string())
                } else {
                    None
                };
                Some(ContentPart::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                    provider_executed: *provider_executed,
                    signature: sig,
                })
            }
            other => Some(other.clone()),
        })
        .collect();
    cloned
}

fn transform_for_qwen(msg: &Message) -> Message {
    // Invariant #3: strip all foreign provider metadata, signatures, and redacted blocks.
    let mut cloned = msg.clone();
    cloned.parts = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Reasoning { .. } => None,
            ContentPart::Text { text, .. } => Some(ContentPart::Text {
                text: text.clone(),
                provider_meta: None,
            }),
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                provider_executed,
                ..
            } => Some(ContentPart::ToolCall {
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
                provider_executed: *provider_executed,
                signature: None,
            }),
            other => Some(other.clone()),
        })
        .collect();
    cloned
}

/// Drops trailing unsettled assistant tool calls after the last settled local tool so retries
/// do not re-execute already-settled tools or leave dangling call_ids.
fn trim_incomplete_trailing_assistant(messages: &mut [Message]) {
    let settled_ids: HashSet<String> = messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .filter_map(|p| match p {
            ContentPart::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    if let Some(last) = messages.last_mut()
        && last.role == Role::Assistant
    {
        let mut last_settled_pos = None;
        let mut has_unsettled = false;
        for (i, part) in last.parts.iter().enumerate() {
            if let ContentPart::ToolCall {
                id,
                provider_executed: false,
                ..
            } = part
            {
                if settled_ids.contains(id) {
                    last_settled_pos = Some(i);
                } else {
                    has_unsettled = true;
                }
            }
        }
        if has_unsettled && let Some(pos) = last_settled_pos {
            last.parts.truncate(pos + 1);
        }
    }
}
