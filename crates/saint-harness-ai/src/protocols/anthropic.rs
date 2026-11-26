use crate::client::prepare_request;
use crate::error_classifier::{classify_http_error, classify_transport_error};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{StreamExt, stream};
use saint_harness_schema::{
    AiError, ContentPart, FinishReason, LLMEvent, LlmRequest, LlmStream, Message, Role, ToolChoice,
    ToolInputErrorKind, ToolResultBlock, TransportOp, Usage, WireProtocol,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub struct AnthropicMessagesProtocol {
    http: reqwest::Client,
}

impl Default for AnthropicMessagesProtocol {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl AnthropicMessagesProtocol {
    pub fn build_payload(req: &LlmRequest) -> Result<Value, serde_json::Error> {
        let prepared = prepare_request(req.clone())?;
        let req = &prepared.request;
        let plan = &prepared.cache_plan;

        let mut system_blocks = Vec::new();
        for (idx, s) in req.system.iter().enumerate() {
            let mut blk = Map::new();
            blk.insert("type".to_string(), json!("text"));
            blk.insert("text".to_string(), json!(s));
            if plan.mark_system_indices.contains(&idx) {
                blk.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
            system_blocks.push(Value::Object(blk));
        }

        let mut tools_arr = Vec::new();
        let tool_len = req.tools.len();
        for (idx, t) in req.tools.iter().enumerate() {
            let mut obj = Map::new();
            obj.insert("name".to_string(), json!(t.name));
            obj.insert("description".to_string(), json!(t.description));
            obj.insert("input_schema".to_string(), t.input_schema.clone());
            if t.defer_loading && req.profile.tools.defer_loading {
                obj.insert("defer_loading".to_string(), json!(true));
            }
            if plan.mark_last_tool && idx + 1 == tool_len {
                obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
            }
            tools_arr.push(Value::Object(obj));
        }

        let mut msgs_arr = Vec::new();
        let msg_len = req.messages.len();
        for (idx, msg) in req.messages.iter().enumerate() {
            let is_tail = plan.mark_tail_message && idx + 1 == msg_len;
            msgs_arr.push(encode_anthropic_message(msg, is_tail));
        }

        let mut body = Map::new();
        body.insert("model".to_string(), json!(req.model.model_id));
        body.insert("max_tokens".to_string(), json!(req.max_output_tokens));
        body.insert("stream".to_string(), json!(true));
        if !system_blocks.is_empty() {
            body.insert("system".to_string(), Value::Array(system_blocks));
        }
        body.insert("messages".to_string(), Value::Array(msgs_arr));
        if !tools_arr.is_empty() {
            body.insert("tools".to_string(), Value::Array(tools_arr));
            let tc = match req.tool_choice {
                ToolChoice::Auto => json!({ "type": "auto" }),
                ToolChoice::None => json!({ "type": "none" }),
                ToolChoice::Required => json!({ "type": "any" }),
            };
            body.insert("tool_choice".to_string(), tc);
        }

        if req.profile.reasoning.mode != "disabled" && !req.smol_mode {
            if req.model.supports_adaptive_thinking {
                body.insert("thinking".to_string(), json!({ "type": "adaptive" }));
                if let Some(ref effort) = req.profile.reasoning.effort {
                    body.insert("output_config".to_string(), json!({ "effort": effort }));
                }
            } else {
                let half = (req.max_output_tokens / 2).max(1024);
                let budget = half.clamp(1024, half);
                body.insert(
                    "thinking".to_string(),
                    json!({ "type": "enabled", "budget_tokens": budget }),
                );
            }
        }

        if req.profile.tools.clear_tool_uses_server
            && let Some(ref strategy_type) = req.route.clear_tool_uses_type
        {
            body.insert(
                "context_management".to_string(),
                json!({
                    "edits": [{ "type": strategy_type }]
                }),
            );
        }

        Ok(Value::Object(body))
    }
}

fn encode_anthropic_message(msg: &Message, mark_tail_cache: bool) -> Value {
    let role = match msg.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    let mut blocks = Vec::new();
    for part in &msg.parts {
        match part {
            ContentPart::Text { text, .. } => {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            ContentPart::Reasoning {
                text,
                signature,
                redacted_data,
                ..
            } => {
                if let Some(data) = redacted_data {
                    blocks.push(json!({ "type": "redacted_thinking", "data": data }));
                } else if let Some(sig) = signature {
                    blocks.push(json!({
                        "type": "thinking",
                        "thinking": text,
                        "signature": sig
                    }));
                }
            }
            ContentPart::Media {
                mime_type,
                base64_data,
                ..
            } => {
                blocks.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": base64_data
                    }
                }));
            }
            ContentPart::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": arguments
                }));
            }
            ContentPart::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } => {
                let items: Vec<Value> = content
                    .iter()
                    .map(|b| match b {
                        ToolResultBlock::Text { text } => json!({ "type": "text", "text": text }),
                        ToolResultBlock::Media {
                            mime_type,
                            base64_data,
                        } => json!({
                            "type": "image",
                            "source": { "type": "base64", "media_type": mime_type, "data": base64_data }
                        }),
                    })
                    .collect();
                blocks.push(json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": items,
                    "is_error": is_error
                }));
            }
            ContentPart::Compaction {
                summary_xml: Some(s),
                ..
            } => {
                blocks.push(json!({ "type": "text", "text": s }));
            }
            _ => {}
        }
    }

    if mark_tail_cache && let Some(Value::Object(last)) = blocks.last_mut() {
        last.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
    }

    json!({ "role": role, "content": blocks })
}

#[derive(Default)]
struct ActiveBlock {
    kind: String,
    id: String,
    name: String,
    json_buf: String,
    signature: Option<String>,
    redacted: Option<String>,
}

#[derive(Default)]
pub struct AnthropicSseDecoder {
    blocks: BTreeMap<usize, ActiveBlock>,
    usage: Usage,
    finish_reason: Option<FinishReason>,
    saw_stop: bool,
    refusal_details: Option<String>,
}

impl AnthropicSseDecoder {
    pub fn push_event(&mut self, event_name: &str, data: &str) -> Vec<Result<LLMEvent, AiError>> {
        let mut out = Vec::new();
        let val: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let ev_type = val
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or(event_name);

        match ev_type {
            "message_start" => {
                if let Some(u) = val
                    .get("message")
                    .and_then(|m| m.get("usage"))
                    .and_then(|v| v.as_object())
                {
                    self.apply_usage(u);
                }
            }
            "content_block_start" => {
                let idx = val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(cb) = val.get("content_block") {
                    let kind = cb
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let mut active = ActiveBlock {
                        kind: kind.clone(),
                        ..ActiveBlock::default()
                    };
                    match kind.as_str() {
                        "text" => out.push(Ok(LLMEvent::TextStart)),
                        "thinking" => out.push(Ok(LLMEvent::ReasoningStart)),
                        "redacted_thinking" => {
                            active.redacted = cb
                                .get("data")
                                .and_then(|v| v.as_str())
                                .map(ToString::to_string);
                            out.push(Ok(LLMEvent::ReasoningStart));
                        }
                        "tool_use" => {
                            active.id = cb
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            active.name = cb
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            out.push(Ok(LLMEvent::ToolInputStart {
                                id: active.id.clone(),
                                name: active.name.clone(),
                            }));
                        }
                        _ => {}
                    }
                    self.blocks.insert(idx, active);
                }
            }
            "content_block_delta" => {
                let idx = val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(delta) = val.get("delta") {
                    let dtype = delta
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    match dtype {
                        "text_delta" => {
                            if let Some(t) = delta.get("text").and_then(|v| v.as_str()) {
                                out.push(Ok(LLMEvent::TextDelta {
                                    delta: t.to_string(),
                                }));
                            }
                        }
                        "thinking_delta" => {
                            if let Some(t) = delta.get("thinking").and_then(|v| v.as_str()) {
                                out.push(Ok(LLMEvent::ReasoningDelta {
                                    delta: t.to_string(),
                                }));
                            }
                        }
                        "signature_delta" => {
                            if let Some(sig) = delta.get("signature").and_then(|v| v.as_str())
                                && let Some(blk) = self.blocks.get_mut(&idx)
                            {
                                blk.signature.get_or_insert_with(String::new).push_str(sig);
                            }
                        }
                        "input_json_delta" => {
                            if let Some(pj) = delta.get("partial_json").and_then(|v| v.as_str())
                                && let Some(blk) = self.blocks.get_mut(&idx)
                            {
                                blk.json_buf.push_str(pj);
                                out.push(Ok(LLMEvent::ToolInputDelta {
                                    id: blk.id.clone(),
                                    delta: pj.to_string(),
                                }));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "content_block_stop" => {
                let idx = val.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(blk) = self.blocks.remove(&idx) {
                    match blk.kind.as_str() {
                        "text" => out.push(Ok(LLMEvent::TextEnd)),
                        "thinking" | "redacted_thinking" => {
                            out.push(Ok(LLMEvent::ReasoningEnd {
                                signature: blk.signature,
                                redacted: blk.redacted,
                            }));
                        }
                        "tool_use" => {
                            let parsed = if blk.json_buf.trim().is_empty() {
                                Ok(Value::Object(Map::new()))
                            } else {
                                serde_json::from_str::<Value>(&blk.json_buf)
                            };
                            match parsed {
                                Ok(arguments) => out.push(Ok(LLMEvent::ToolCall {
                                    id: blk.id,
                                    name: blk.name,
                                    arguments,
                                    provider_executed: false,
                                    signature: None,
                                })),
                                Err(_) => out.push(Ok(LLMEvent::ToolInputError {
                                    id: blk.id,
                                    name: blk.name,
                                    raw_excerpt: blk.json_buf.chars().take(2048).collect(),
                                    reason: ToolInputErrorKind::MalformedJson,
                                })),
                            }
                        }
                        _ => {}
                    }
                }
            }
            "message_delta" => {
                if let Some(u) = val.get("usage").and_then(|v| v.as_object()) {
                    self.apply_usage(u);
                }
                if let Some(delta) = val.get("delta") {
                    if let Some(sr) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                        if sr == "refusal" {
                            let details = delta
                                .get("stop_details")
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| {
                                    "provider returned stop_reason=refusal".to_string()
                                });
                            self.refusal_details = Some(details);
                            self.finish_reason = Some(FinishReason::ContentFilter);
                        } else {
                            self.finish_reason = Some(match sr {
                                "end_turn" | "stop_sequence" => FinishReason::Stop,
                                "tool_use" => FinishReason::ToolCalls,
                                "max_tokens" => FinishReason::Length,
                                _ => FinishReason::Unknown,
                            });
                        }
                    }
                }
            }
            "message_stop" => {
                self.saw_stop = true;
                if let Some(details) = self.refusal_details.take() {
                    out.push(Err(AiError::ContentPolicy { message: details }));
                } else {
                    out.push(Ok(LLMEvent::Finish {
                        reason: self.finish_reason.unwrap_or(FinishReason::Unknown),
                        usage: self.usage.clone(),
                    }));
                }
            }
            "error" => {
                let msg = val
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("anthropic stream error")
                    .to_string();
                out.push(Err(AiError::ProviderInternal {
                    status: None,
                    message: msg,
                    retry_after_ms: None,
                    should_retry_header: None,
                }));
            }
            _ => {}
        }

        out
    }

    pub fn on_halt(&mut self) -> Vec<Result<LLMEvent, AiError>> {
        if self.saw_stop {
            return Vec::new();
        }
        vec![Err(AiError::IncompleteStream {
            message: "Anthropic SSE stream terminated before message_stop".to_string(),
        })]
    }

    fn apply_usage(&mut self, u: &Map<String, Value>) {
        if let Some(v) = u.get("input_tokens").and_then(|v| v.as_u64()) {
            self.usage.non_cached_input_tokens = v;
        }
        if let Some(v) = u.get("cache_read_input_tokens").and_then(|v| v.as_u64()) {
            self.usage.cache_read_input_tokens = v;
        }
        if let Some(v) = u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
        {
            self.usage.cache_write_input_tokens = v;
        }
        if let Some(v) = u.get("output_tokens").and_then(|v| v.as_u64()) {
            self.usage.output_tokens = v;
        }
        self.usage.input_tokens = self
            .usage
            .non_cached_input_tokens
            .saturating_add(self.usage.cache_read_input_tokens)
            .saturating_add(self.usage.cache_write_input_tokens);
    }
}

#[async_trait]
impl WireProtocol for AnthropicMessagesProtocol {
    fn protocol_id(&self) -> &str {
        "anthropic_messages"
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, AiError> {
        let payload = Self::build_payload(&request).map_err(|e| AiError::InvalidRequest {
            message: e.to_string(),
        })?;
        let url = format!(
            "{}/v1/messages",
            request.route.base_url.trim_end_matches('/')
        );
        let mut req_builder = self
            .http
            .post(&url)
            .header("anthropic-version", "2023-06-01")
            .json(&payload);
        if !request.route.beta_headers.is_empty() {
            req_builder =
                req_builder.header("anthropic-beta", request.route.beta_headers.join(","));
        }
        if let Some(key) = saint_harness_schema::resolve_api_key(&request.route.api_key_env) {
            req_builder = req_builder.header("x-api-key", key);
        }
        for (k, v) in &request.extra_headers {
            req_builder = req_builder.header(k, v);
        }

        let resp = req_builder
            .send()
            .await
            .map_err(|e| classify_transport_error(&e.to_string(), TransportOp::Connect, false))?;
        let status = resp.status().as_u16();
        if status >= 400 {
            let mut headers = BTreeMap::new();
            for (k, v) in resp.headers() {
                if let Ok(s) = v.to_str() {
                    headers.insert(k.as_str().to_string(), s.to_string());
                }
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &headers, &body));
        }

        let sse = resp.bytes_stream().eventsource();
        let out_stream = stream::unfold(
            (
                sse,
                AnthropicSseDecoder::default(),
                std::collections::VecDeque::from([Ok(LLMEvent::StepStart)]),
                false,
            ),
            |(mut sse, mut decoder, mut buf, mut done)| async move {
                loop {
                    if let Some(item) = buf.pop_front() {
                        return Some((item, (sse, decoder, buf, done)));
                    }
                    if done {
                        return None;
                    }
                    match sse.next().await {
                        Some(Ok(ev)) => {
                            for out in decoder.push_event(&ev.event, &ev.data) {
                                buf.push_back(out);
                            }
                        }
                        Some(Err(e)) => {
                            done = true;
                            buf.push_back(Err(classify_transport_error(
                                &e.to_string(),
                                TransportOp::Receive,
                                true,
                            )));
                        }
                        None => {
                            done = true;
                            for out in decoder.on_halt() {
                                buf.push_back(out);
                            }
                        }
                    }
                }
            },
        );
        Ok(Box::pin(out_stream))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{ResolvedModel, ToolDef, embedded_profile, embedded_routes};

    #[test]
    fn cassette_adaptive_thinking_redacted_and_usage_sum() {
        let mut dec = AnthropicSseDecoder::default();
        let mut events = Vec::new();
        let seq = [
            (
                "message_start",
                r#"{"type":"message_start","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":800,"cache_creation_input_tokens":200}}}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_abc"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":0}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"opaque=="}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":1}"#,
            ),
            (
                "content_block_start",
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu_1","name":"read"}}"#,
            ),
            (
                "content_block_delta",
                r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.rs\"}"}}"#,
            ),
            (
                "content_block_stop",
                r#"{"type":"content_block_stop","index":2}"#,
            ),
            (
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":75}}"#,
            ),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ];
        for (ev, data) in seq {
            events.extend(dec.push_event(ev, data));
        }
        assert!(events.iter().any(|e| matches!(
            e,
            Ok(LLMEvent::ReasoningEnd { signature: Some(s), .. }) if s == "sig_abc"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            Ok(LLMEvent::ReasoningEnd { redacted: Some(r), .. }) if r == "opaque=="
        )));
        let finish = events.iter().find_map(|e| match e {
            Ok(LLMEvent::Finish { usage, reason }) => Some((*reason, usage.clone())),
            _ => None,
        });
        let (reason, usage) = finish.expect("finish");
        assert_eq!(reason, FinishReason::ToolCalls);
        assert_eq!(usage.input_tokens, 1100);
        assert_eq!(usage.output_tokens, 75);
    }

    #[test]
    fn cassette_http_200_refusal_maps_to_content_policy() {
        let mut dec = AnthropicSseDecoder::default();
        let mut events = Vec::new();
        events.extend(dec.push_event(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"reason":"provider_policy"}}}"#,
        ));
        events.extend(dec.push_event("message_stop", r#"{"type":"message_stop"}"#));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Err(AiError::ContentPolicy { .. })))
        );
    }

    #[test]
    fn payload_enforces_max_4_cache_breakpoints_and_defer_loading() {
        let profile = embedded_profile("claude-flagship").unwrap();
        let routes = embedded_routes().unwrap();
        let mut route = routes.routes.get("anthropic_direct").unwrap().clone();
        route.clear_tool_uses_type = Some("clear_tool_uses_auto".to_string());
        let req = LlmRequest {
            system: vec!["s0".into(), "s1".into(), "s2".into()],
            messages: vec![Message {
                id: "m1".into(),
                parent_msg_id: None,
                role: Role::User,
                parts: vec![ContentPart::Text {
                    text: "hi".into(),
                    provider_meta: None,
                }],
                model_provider: None,
                model_id: None,
            }],
            tools: vec![
                ToolDef {
                    name: "a".into(),
                    description: "a".into(),
                    input_schema: json!({"type":"object"}),
                    defer_loading: true,
                    server_name: None,
                },
                ToolDef {
                    name: "b".into(),
                    description: "b".into(),
                    input_schema: json!({"type":"object"}),
                    defer_loading: false,
                    server_name: None,
                },
            ],
            tool_choice: ToolChoice::Auto,
            max_output_tokens: 8192,
            model: ResolvedModel {
                provider: "anthropic".into(),
                model_id: "claude-test".into(),
                display_name: "Claude".into(),
                native_window: 200_000,
                effective_window: 170_000,
                max_output_tokens: 32_000,
                supports_adaptive_thinking: true,
                supports_vision: true,
                resolution_source: "test".into(),
            },
            profile,
            route,
            session_affinity: "s1".into(),
            smol_mode: false,
            extra_headers: BTreeMap::new(),
        };
        let payload = AnthropicMessagesProtocol::build_payload(&req).unwrap();
        let raw = serde_json::to_string(&payload).unwrap();
        let bp_count = raw.matches("cache_control").count();
        assert_eq!(bp_count, 4);
        assert_eq!(payload["tools"][0]["defer_loading"], json!(true));
        assert_eq!(
            payload["context_management"]["edits"][0]["type"],
            json!("clear_tool_uses_auto")
        );
    }
}
