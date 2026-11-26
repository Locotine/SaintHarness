use crate::cache_policy::{CacheBreakpointPlan, CachePolicyKind, plan_cache_breakpoints};
use crate::canonical::{canonical_blake3, canonicalize_json, sanitize_json_strings, sanitize_text};
use crate::schema_projector::project_schema;
use saint_harness_schema::{ContentPart, LlmRequest, Message, ToolDef};

#[derive(Debug, Clone)]
pub struct PreparedRequest {
    pub request: LlmRequest,
    pub cache_plan: CacheBreakpointPlan,
    pub prefix_hash: String,
}

/// Runs the deterministic 5-step request preparation pipeline:
/// 1. sanitize invalid text/surrogates
/// 2. sort tools by name ascending
/// 3. project schemas (when strict schema sanitizer enabled or Gemini protocol)
/// 4. canonicalize all JSON values recursively
/// 5. plan cache breakpoints and compute deterministic prefix blake3 hash
pub fn prepare_request(mut req: LlmRequest) -> Result<PreparedRequest, serde_json::Error> {
    req.system = req.system.iter().map(|s| sanitize_text(s)).collect();
    req.messages = req.messages.into_iter().map(sanitize_message).collect();

    req.tools.sort_by(|a, b| a.name.cmp(&b.name));
    let strict_schema = req.profile.tools.strict_schema_sanitizer
        || req.route.protocol == "gemini_generate_content";
    req.tools = req
        .tools
        .into_iter()
        .map(|mut t| {
            t.description = sanitize_text(&t.description);
            let sanitized = sanitize_json_strings(&t.input_schema);
            t.input_schema = if strict_schema {
                project_schema(&sanitized)
            } else {
                canonicalize_json(&sanitized)
            };
            t
        })
        .collect();

    let policy = CachePolicyKind::parse(&req.profile.cache.policy);
    let cache_plan = plan_cache_breakpoints(
        policy,
        req.tools.len(),
        req.system.len(),
        req.messages.len(),
    );

    let prefix_payload = serde_json::json!({
        "system": &req.system,
        "tools": &req.tools,
    });
    let prefix_hash = canonical_blake3(&prefix_payload)?;

    Ok(PreparedRequest {
        request: req,
        cache_plan,
        prefix_hash,
    })
}

fn sanitize_message(mut msg: Message) -> Message {
    msg.parts = msg
        .parts
        .into_iter()
        .map(|part| match part {
            ContentPart::Text {
                text,
                provider_meta,
            } => ContentPart::Text {
                text: sanitize_text(&text),
                provider_meta,
            },
            ContentPart::Reasoning {
                text,
                signature,
                redacted_data,
                provider_meta,
            } => ContentPart::Reasoning {
                text: sanitize_text(&text),
                signature,
                redacted_data,
                provider_meta,
            },
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                provider_executed,
                signature,
            } => ContentPart::ToolCall {
                id,
                name,
                arguments: canonicalize_json(&sanitize_json_strings(&arguments)),
                provider_executed,
                signature,
            },
            other => other,
        })
        .collect();
    msg
}

pub fn sort_tools(tools: &mut [ToolDef]) {
    tools.sort_by(|a, b| a.name.cmp(&b.name));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{ResolvedModel, ToolChoice, embedded_profile, embedded_routes};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn prepare_request_sorts_tools_and_stabilizes_prefix_hash() {
        let profile = embedded_profile("claude-flagship").unwrap();
        let routes = embedded_routes().unwrap();
        let route = routes.routes.get("anthropic_direct").unwrap().clone();
        let model = ResolvedModel {
            provider: "anthropic".to_string(),
            model_id: "claude-test".to_string(),
            display_name: "Claude Test".to_string(),
            native_window: 200_000,
            effective_window: 170_000,
            max_output_tokens: 32_000,
            supports_adaptive_thinking: true,
            supports_vision: true,
            resolution_source: "test".to_string(),
        };

        let tool_a = ToolDef {
            name: "read".to_string(),
            description: "read file".to_string(),
            input_schema: json!({"type":"object","properties":{"z":{"type":"string"},"a":{"type":"number"}}}),
            defer_loading: false,
            server_name: None,
        };
        let tool_b = ToolDef {
            name: "edit".to_string(),
            description: "edit file".to_string(),
            input_schema: json!({"properties":{"b":{"type":"string"},"a":{"type":"string"}},"type":"object"}),
            defer_loading: false,
            server_name: None,
        };

        let req1 = LlmRequest {
            system: vec!["sys0".to_string(), "sys1".to_string()],
            messages: vec![],
            tools: vec![tool_a.clone(), tool_b.clone()],
            tool_choice: ToolChoice::Auto,
            max_output_tokens: 4096,
            model: model.clone(),
            profile: profile.clone(),
            route: route.clone(),
            session_affinity: "s1".to_string(),
            smol_mode: false,
            extra_headers: BTreeMap::new(),
        };
        let req2 = LlmRequest {
            tools: vec![tool_b, tool_a],
            ..req1.clone()
        };

        let p1 = prepare_request(req1).unwrap();
        let p2 = prepare_request(req2).unwrap();
        assert_eq!(p1.request.tools[0].name, "edit");
        assert_eq!(p1.request.tools[1].name, "read");
        assert_eq!(p1.prefix_hash, p2.prefix_hash);
        assert!(p1.cache_plan.total_breakpoints() <= 4);
    }
}
