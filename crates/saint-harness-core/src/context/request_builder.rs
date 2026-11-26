use crate::context::to_llm::to_llm_messages;
use saint_harness_schema::{
    ContentPart, LlmRequest, Message, ModelProfile, ResolvedModel, RouteDescriptor, ToolChoice,
    ToolDef,
};
use saint_harness_token::compute_output_limit;
use std::collections::BTreeMap;

pub const IMAGE_BUDGET_HIGH_BYTES: usize = 25 * 1024 * 1024;
pub const IMAGE_BUDGET_LOW_BYTES: usize = 15 * 1024 * 1024;

pub struct RequestBuildInput<'a> {
    pub system: Vec<String>,
    pub history: &'a [Message],
    pub tools: Vec<ToolDef>,
    pub tool_choice: ToolChoice,
    pub model: ResolvedModel,
    pub profile: ModelProfile,
    pub route: RouteDescriptor,
    pub measured_input_tokens: u64,
    pub estimated_delta_tokens: u64,
    pub session_id: &'a str,
    pub parent_id: Option<&'a str>,
    pub fork_source_id: Option<&'a str>,
    pub smol_mode: bool,
}

pub fn build_llm_request(input: RequestBuildInput<'_>) -> LlmRequest {
    let mut messages = to_llm_messages(input.history, &input.model.provider, &input.model.model_id);
    bound_images_hysteresis(
        &mut messages,
        IMAGE_BUDGET_HIGH_BYTES,
        IMAGE_BUDGET_LOW_BYTES,
    );

    let output_limit = compute_output_limit(
        input.model.max_output_tokens,
        input.model.effective_window,
        input.measured_input_tokens,
        input.estimated_delta_tokens,
    );

    let affinity = input
        .parent_id
        .or(input.fork_source_id)
        .unwrap_or(input.session_id)
        .to_string();

    LlmRequest {
        system: input.system,
        messages,
        tools: input.tools,
        tool_choice: input.tool_choice,
        max_output_tokens: output_limit,
        model: input.model,
        profile: input.profile,
        route: input.route,
        session_affinity: affinity,
        smol_mode: input.smol_mode,
        extra_headers: BTreeMap::new(),
    }
}

/// Drops oldest base64 `Media` parts only when total exceeds `high_bytes` (25 MiB),
/// trimming down to `low_bytes` (15 MiB) to preserve prompt prefix cache stability.
pub fn bound_images_hysteresis(messages: &mut [Message], high_bytes: usize, low_bytes: usize) {
    let mut total_bytes: usize = messages
        .iter()
        .flat_map(|m| m.parts.iter())
        .map(|p| match p {
            ContentPart::Media { base64_data, .. } => base64_data.len(),
            _ => 0,
        })
        .sum();
    if total_bytes <= high_bytes {
        return;
    }

    for msg in messages.iter_mut() {
        for part in msg.parts.iter_mut() {
            if total_bytes <= low_bytes {
                return;
            }
            if let ContentPart::Media {
                base64_data,
                source_path,
                ..
            } = part
            {
                total_bytes = total_bytes.saturating_sub(base64_data.len());
                *part = ContentPart::Text {
                    text: format!(
                        "[older image pruned for budget: {}]",
                        source_path.as_deref().unwrap_or("inline")
                    ),
                    provider_meta: None,
                };
            }
        }
    }
}
