use crate::client::prepare_request;
use crate::error_classifier::{classify_http_error, classify_transport_error};
use crate::protocols::qwen_guard::{
    ThinkSplitState, ensure_unique_tool_call_id, repair_json_arguments, rescue_embedded_tool_calls,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{StreamExt, stream};
use saint_harness_schema::{
    AiError, ContentPart, FinishReason, LLMEvent, LlmRequest, LlmStream, Message, Role, ToolChoice,
    ToolInputErrorKind, ToolResultBlock, TransportOp, Usage, WireProtocol,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};

pub struct OpenAiChatCompatProtocol {
    http: reqwest::Client,
}

impl Default for OpenAiChatCompatProtocol {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl OpenAiChatCompatProtocol {
    pub fn build_payload(req: &LlmRequest) -> Result<Value, serde_json::Error> {
        let prepared = prepare_request(req.clone())?;
        let req = &prepared.request;
        let mut messages = Vec::new();
        for (idx, sys) in req.system.iter().enumerate() {
            let mut obj = Map::new();
            obj.insert("role".to_string(), json!("system"));
            if prepared.cache_plan.mark_system_indices.contains(&idx) {
                obj.insert(
                    "content".to_string(),
                    json!([{
                        "type": "text",
                        "text": sys,
                        "cache_control": { "type": "ephemeral" }
                    }]),
                );
            } else {
                obj.insert("content".to_string(), json!(sys));
            }
            messages.push(Value::Object(obj));
        }

        for msg in &req.messages {
            messages.extend(encode_openai_message(msg));
        }

        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    }
                })
            })
            .collect();

        let mut body = Map::new();
        body.insert("model".to_string(), json!(req.model.model_id));
        body.insert("messages".to_string(), Value::Array(messages));
        body.insert("stream".to_string(), json!(true));
        body.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
        body.insert("max_tokens".to_string(), json!(req.max_output_tokens));

        if !tools.is_empty() {
            body.insert("tools".to_string(), Value::Array(tools));
            let tc = match req.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Required => json!("required"),
            };
            body.insert("tool_choice".to_string(), tc);
        }

        if let Some(ref sampling) = req.profile.sampling {
            if let Some(t) = sampling.temperature {
                body.insert("temperature".to_string(), json!(t));
            }
            if let Some(p) = sampling.top_p {
                body.insert("top_p".to_string(), json!(p));
            }
            if let Some(pp) = sampling.presence_penalty {
                body.insert("presence_penalty".to_string(), json!(pp));
            }
        }

        let enable_thinking = !(req.smol_mode && req.profile.reasoning.smol_disables_thinking)
            && req.profile.reasoning.mode != "disabled";
        body.insert(
            "chat_template_kwargs".to_string(),
            json!({
                "enable_thinking": enable_thinking,
                "preserve_thinking": req.profile.reasoning.preserve_thinking
            }),
        );

        Ok(Value::Object(body))
    }
}

fn encode_openai_message(msg: &Message) -> Vec<Value> {
    let mut out = Vec::new();
    match msg.role {
        Role::User | Role::Synthetic | Role::System | Role::Compaction => {
            let role = if msg.role == Role::System {
                "system"
            } else {
                "user"
            };
            let mut text = String::new();
            for part in &msg.parts {
                match part {
                    ContentPart::Text { text: t, .. } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    ContentPart::Compaction {
                        summary_xml: Some(s),
                        ..
                    } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(s);
                    }
                    _ => {}
                }
            }
            out.push(json!({ "role": role, "content": text }));
        }
        Role::Assistant => {
            let mut content = String::new();
            let mut tool_calls = Vec::new();
            for part in &msg.parts {
                match part {
                    ContentPart::Text { text, .. } => content.push_str(text),
                    ContentPart::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } => {
                        let args_str =
                            serde_json::to_string(arguments).unwrap_or_else(|_| "{}".to_string());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": args_str }
                        }));
                    }
                    _ => {}
                }
            }
            let mut obj = Map::new();
            obj.insert("role".to_string(), json!("assistant"));
            obj.insert("content".to_string(), json!(content));
            if !tool_calls.is_empty() {
                obj.insert("tool_calls".to_string(), Value::Array(tool_calls));
            }
            out.push(Value::Object(obj));
        }
        Role::Tool => {
            for part in &msg.parts {
                if let ContentPart::ToolResult {
                    call_id, content, ..
                } = part
                {
                    let body = content
                        .iter()
                        .filter_map(|b| match b {
                            ToolResultBlock::Text { text } => Some(text.as_str()),
                            ToolResultBlock::Media { .. } => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": body
                    }));
                }
            }
        }
    }
    out
}

#[derive(Default)]
struct PendingToolAcc {
    id: String,
    name: String,
    args_buf: String,
}

#[derive(Default)]
pub struct OpenAiSseDecoder {
    think_split: ThinkSplitState,
    in_text: bool,
    in_reasoning: bool,
    pending_tools: BTreeMap<usize, PendingToolAcc>,
    seen_tool_ids: HashSet<String>,
    usage: Usage,
    finish_reason: Option<FinishReason>,
    saw_done: bool,
    raw_visible: String,
    enable_thinking_guard: bool,
    enable_json_repair: bool,
    emitted_tool_calls: usize,
}

impl OpenAiSseDecoder {
    pub fn new(enable_thinking_guard: bool, enable_json_repair: bool) -> Self {
        Self {
            enable_thinking_guard,
            enable_json_repair,
            ..Self::default()
        }
    }

    pub fn push_data(&mut self, data: &str) -> Vec<Result<LLMEvent, AiError>> {
        let mut events = Vec::new();
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return events;
        }
        if trimmed == "[DONE]" {
            self.saw_done = true;
            events.extend(self.finalize(false));
            return events;
        }

        let val: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return events,
        };

        if let Some(u) = val.get("usage").and_then(|v| v.as_object()) {
            let prompt = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cached = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let completion = u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let reasoning = u
                .get("completion_tokens_details")
                .and_then(|d| d.get("reasoning_tokens"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.usage = Usage {
                input_tokens: prompt,
                non_cached_input_tokens: prompt.saturating_sub(cached),
                cache_read_input_tokens: cached,
                cache_write_input_tokens: 0,
                output_tokens: completion,
                reasoning_tokens: reasoning,
                provider_reported_cost_usd: None,
            };
        }

        if let Some(choice) = val
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(r) = delta.get("reasoning_content").and_then(|v| v.as_str())
                    && !r.is_empty()
                {
                    if !self.in_reasoning {
                        self.in_reasoning = true;
                        events.push(Ok(LLMEvent::ReasoningStart));
                    }
                    self.think_split.captured_think.push_str(r);
                    events.push(Ok(LLMEvent::ReasoningDelta {
                        delta: r.to_string(),
                    }));
                }

                if let Some(c) = delta.get("content").and_then(|v| v.as_str())
                    && !c.is_empty()
                {
                    let (vis, reas) = self.think_split.push(c);
                    if !reas.is_empty() {
                        if !self.in_reasoning {
                            self.in_reasoning = true;
                            events.push(Ok(LLMEvent::ReasoningStart));
                        }
                        events.push(Ok(LLMEvent::ReasoningDelta { delta: reas }));
                    }
                    if !vis.is_empty() {
                        if self.in_reasoning {
                            self.in_reasoning = false;
                            events.push(Ok(LLMEvent::ReasoningEnd {
                                signature: None,
                                redacted: None,
                            }));
                        }
                        if !self.in_text {
                            self.in_text = true;
                            events.push(Ok(LLMEvent::TextStart));
                        }
                        self.raw_visible.push_str(&vis);
                        events.push(Ok(LLMEvent::TextDelta { delta: vis }));
                    }
                }

                if let Some(tcs) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        let entry = self.pending_tools.entry(idx).or_default();
                        let was_empty = entry.name.is_empty();
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str())
                            && !id.is_empty()
                            && entry.id.is_empty()
                        {
                            entry.id =
                                ensure_unique_tool_call_id(Some(id), &mut self.seen_tool_ids);
                        }
                        if let Some(f) = tc.get("function") {
                            if let Some(name) = f.get("name").and_then(|v| v.as_str())
                                && !name.is_empty()
                            {
                                entry.name.push_str(name);
                            }
                            if was_empty && !entry.name.is_empty() {
                                if entry.id.is_empty() {
                                    entry.id =
                                        ensure_unique_tool_call_id(None, &mut self.seen_tool_ids);
                                }
                                events.push(Ok(LLMEvent::ToolInputStart {
                                    id: entry.id.clone(),
                                    name: entry.name.clone(),
                                }));
                            }
                            if let Some(args) = f.get("arguments").and_then(|v| v.as_str())
                                && !args.is_empty()
                            {
                                entry.args_buf.push_str(args);
                                events.push(Ok(LLMEvent::ToolInputDelta {
                                    id: entry.id.clone(),
                                    delta: args.to_string(),
                                }));
                            }
                        }
                    }
                }
            }

            if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                self.finish_reason = Some(match fr {
                    "stop" => FinishReason::Stop,
                    "tool_calls" | "function_call" => FinishReason::ToolCalls,
                    "length" => FinishReason::Length,
                    "content_filter" => FinishReason::ContentFilter,
                    _ => FinishReason::Unknown,
                });
            }
        }

        events
    }

    pub fn on_halt(&mut self) -> Vec<Result<LLMEvent, AiError>> {
        if self.saw_done {
            return Vec::new();
        }
        if self.finish_reason.is_none() {
            return vec![Err(AiError::IncompleteStream {
                message: "OpenAI-compatible SSE stream terminated before [DONE] or finish_reason"
                    .to_string(),
            })];
        }
        self.finalize(true)
    }

    fn finalize(&mut self, from_halt: bool) -> Vec<Result<LLMEvent, AiError>> {
        let mut events = Vec::new();
        let (vis, reas) = self.think_split.flush();
        if !reas.is_empty() {
            if !self.in_reasoning {
                self.in_reasoning = true;
                events.push(Ok(LLMEvent::ReasoningStart));
            }
            events.push(Ok(LLMEvent::ReasoningDelta { delta: reas }));
        }
        if self.in_reasoning {
            self.in_reasoning = false;
            events.push(Ok(LLMEvent::ReasoningEnd {
                signature: None,
                redacted: None,
            }));
        }
        if !vis.is_empty() {
            if !self.in_text {
                self.in_text = true;
                events.push(Ok(LLMEvent::TextStart));
            }
            self.raw_visible.push_str(&vis);
            events.push(Ok(LLMEvent::TextDelta { delta: vis }));
        }
        if self.in_text {
            self.in_text = false;
            events.push(Ok(LLMEvent::TextEnd));
        }

        let tools = std::mem::take(&mut self.pending_tools);
        for (_, acc) in tools {
            let id = if acc.id.is_empty() {
                ensure_unique_tool_call_id(None, &mut self.seen_tool_ids)
            } else {
                acc.id
            };
            let parsed = if acc.args_buf.trim().is_empty() {
                Ok(Value::Object(Map::new()))
            } else if self.enable_json_repair {
                repair_json_arguments(&acc.args_buf)
            } else {
                serde_json::from_str::<Value>(&acc.args_buf)
            };
            match parsed {
                Ok(arguments) => {
                    self.emitted_tool_calls += 1;
                    events.push(Ok(LLMEvent::ToolCall {
                        id,
                        name: acc.name,
                        arguments,
                        provider_executed: false,
                        signature: None,
                    }));
                }
                Err(_) => {
                    let kind = if self.finish_reason == Some(FinishReason::Length) {
                        ToolInputErrorKind::TruncatedByLength
                    } else if from_halt {
                        ToolInputErrorKind::IncompleteStream
                    } else {
                        ToolInputErrorKind::MalformedJson
                    };
                    events.push(Ok(LLMEvent::ToolInputError {
                        id,
                        name: acc.name,
                        raw_excerpt: acc.args_buf.chars().take(2048).collect(),
                        reason: kind,
                    }));
                }
            }
        }

        // Thinking-only guard: if no structured tool calls were emitted, rescue embedded XML/JSON tool calls.
        if self.emitted_tool_calls == 0 && self.enable_thinking_guard {
            let combined = format!("{}\n{}", self.think_split.captured_think, self.raw_visible);
            let rescued = rescue_embedded_tool_calls(&combined, &mut self.seen_tool_ids);
            if !rescued.is_empty() {
                for call in rescued {
                    self.emitted_tool_calls += 1;
                    events.push(Ok(LLMEvent::ToolCall {
                        id: call.id,
                        name: call.name,
                        arguments: call.arguments,
                        provider_executed: false,
                        signature: None,
                    }));
                }
                self.finish_reason = Some(FinishReason::ToolCalls);
            }
        }

        events.push(Ok(LLMEvent::Finish {
            reason: self.finish_reason.unwrap_or(FinishReason::Unknown),
            usage: self.usage.clone(),
        }));
        events
    }
}

#[async_trait]
impl WireProtocol for OpenAiChatCompatProtocol {
    fn protocol_id(&self) -> &str {
        "openai_chat_compat"
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, AiError> {
        let payload = Self::build_payload(&request).map_err(|e| AiError::InvalidRequest {
            message: e.to_string(),
        })?;
        let url = format!(
            "{}/chat/completions",
            request.route.base_url.trim_end_matches('/')
        );
        let mut req_builder = self.http.post(&url).json(&payload);
        if let Some(key) = saint_harness_schema::resolve_api_key(&request.route.api_key_env) {
            req_builder = req_builder.bearer_auth(key);
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

        let guard = request.profile.reasoning.thinking_entrapment_guard;
        let repair = request.profile.tools.json_repair;
        let sse = resp.bytes_stream().eventsource();

        let out_stream = stream::unfold(
            (
                sse,
                OpenAiSseDecoder::new(guard, repair),
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
                            for out in decoder.push_data(&ev.data) {
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
    use saint_harness_schema::{ResolvedModel, embedded_profile, embedded_routes};

    fn run_cassette(chunks: &[&str], guard: bool, repair: bool) -> Vec<Result<LLMEvent, AiError>> {
        let mut dec = OpenAiSseDecoder::new(guard, repair);
        let mut out = Vec::new();
        for c in chunks {
            out.extend(dec.push_data(c));
        }
        out.extend(dec.on_halt());
        out
    }

    #[test]
    fn cassette_clean_stream_and_cached_usage() {
        let events = run_cassette(
            &[
                r#"{"choices":[{"delta":{"reasoning_content":"step 1"}}]}"#,
                r#"{"choices":[{"delta":{"content":"Xin chào "}}]}"#,
                r#"{"choices":[{"delta":{"content":"Việt Nam"},"finish_reason":"stop"}]}"#,
                r#"{"choices":[],"usage":{"prompt_tokens":1000,"prompt_tokens_details":{"cached_tokens":850},"completion_tokens":120,"completion_tokens_details":{"reasoning_tokens":40}}}"#,
                "[DONE]",
            ],
            true,
            true,
        );
        let finish = events.iter().find_map(|e| match e {
            Ok(LLMEvent::Finish { reason, usage }) => Some((*reason, usage.clone())),
            _ => None,
        });
        let (reason, usage) = finish.expect("finish event");
        assert_eq!(reason, FinishReason::Stop);
        assert_eq!(usage.non_cached_input_tokens, 150);
        assert_eq!(usage.cache_read_input_tokens, 850);
        assert_eq!(usage.visible_output_tokens(), 80);
    }

    #[test]
    fn cassette_rescues_trapped_tool_call_in_think() {
        let events = run_cassette(
            &[
                r#"{"choices":[{"delta":{"content":"<think>Let me inspect <tool_call>{\"name\":\"read\",\"arguments\":{\"path\":\"Cargo.toml\"}}</tool_call></think>"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ],
            true,
            true,
        );
        assert!(events.iter().any(|e| matches!(
            e,
            Ok(LLMEvent::ToolCall { name, .. }) if name == "read"
        )));
    }

    #[test]
    fn cassette_repairs_fenced_trailing_comma_json_and_dedups_ids() {
        let events = run_cassette(
            &[
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"dup","function":{"name":"read","arguments":"```json\n{\"path\":\"a.rs\",}\n```"}}]}}]}"#,
                r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"dup","function":{"name":"read","arguments":"{\"path\":\"b.rs\"}"}}]},"finish_reason":"tool_calls"}]}"#,
                "[DONE]",
            ],
            true,
            true,
        );
        let calls: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Ok(LLMEvent::ToolCall { id, arguments, .. }) => {
                    Some((id.clone(), arguments.clone()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].0, calls[1].0);
        assert_eq!(calls[0].1, json!({"path": "a.rs"}));
    }

    #[test]
    fn cassette_abrupt_halt_emits_incomplete_stream() {
        let events = run_cassette(
            &[r#"{"choices":[{"delta":{"content":"half"}}]}"#],
            true,
            true,
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Err(AiError::IncompleteStream { .. })))
        );
    }

    #[test]
    fn smol_mode_disables_thinking_in_payload() {
        let profile = embedded_profile("qwen-3.8-27b").unwrap();
        let routes = embedded_routes().unwrap();
        let route = routes.routes.get("qwen_local_sglang").unwrap().clone();
        let req = LlmRequest {
            system: vec!["sys".to_string()],
            messages: vec![],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            max_output_tokens: 1024,
            model: ResolvedModel {
                provider: "qwen".to_string(),
                model_id: "qwen3.8-27b".to_string(),
                display_name: "Qwen 3.8".to_string(),
                native_window: 262_144,
                effective_window: 110_000,
                max_output_tokens: 16_384,
                supports_adaptive_thinking: false,
                supports_vision: false,
                resolution_source: "exact".to_string(),
            },
            profile,
            route,
            session_affinity: "s1".to_string(),
            smol_mode: true,
            extra_headers: BTreeMap::new(),
        };
        let payload = OpenAiChatCompatProtocol::build_payload(&req).unwrap();
        assert_eq!(
            payload["chat_template_kwargs"]["enable_thinking"],
            json!(false)
        );
    }
}
