use crate::client::prepare_request;
use crate::error_classifier::{classify_http_error, classify_transport_error};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{StreamExt, stream};
use saint_harness_schema::{
    AiError, ContentPart, FinishReason, LLMEvent, LlmRequest, LlmStream, Message, Role, ToolChoice,
    ToolResultBlock, TransportOp, Usage, WireProtocol,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use ulid::Ulid;

const DEFAULT_UNSIGNED_SENTINEL: &str = "skip_thought_signature_validator";

pub struct GeminiGenerateContentProtocol {
    http: reqwest::Client,
}

impl Default for GeminiGenerateContentProtocol {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

impl GeminiGenerateContentProtocol {
    pub fn build_payload(req: &LlmRequest) -> Result<Value, serde_json::Error> {
        let prepared = prepare_request(req.clone())?;
        let req = &prepared.request;
        let sentinel = req
            .profile
            .reasoning
            .unsigned_tool_sentinel
            .as_deref()
            .unwrap_or(DEFAULT_UNSIGNED_SENTINEL);

        let mut body = Map::new();
        if !req.system.is_empty() {
            let parts: Vec<Value> = req.system.iter().map(|s| json!({ "text": s })).collect();
            body.insert("systemInstruction".to_string(), json!({ "parts": parts }));
        }

        let mut contents = Vec::new();
        for msg in &req.messages {
            contents.extend(encode_gemini_message(
                msg,
                sentinel,
                req.profile.tools.separate_user_turn_after_fn_resp,
            ));
        }
        body.insert("contents".to_string(), Value::Array(contents));

        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema
                    })
                })
                .collect();
            body.insert(
                "tools".to_string(),
                json!([{ "functionDeclarations": decls }]),
            );
            let mode = match req.tool_choice {
                ToolChoice::Auto => "AUTO",
                ToolChoice::None => "NONE",
                ToolChoice::Required => "ANY",
            };
            body.insert(
                "toolConfig".to_string(),
                json!({ "functionCallingConfig": { "mode": mode } }),
            );
        }

        body.insert(
            "generationConfig".to_string(),
            json!({ "maxOutputTokens": req.max_output_tokens }),
        );

        if let Some(ref threshold) = req.profile.tools.safety_settings {
            let categories = [
                "HARM_CATEGORY_HARASSMENT",
                "HARM_CATEGORY_HATE_SPEECH",
                "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "HARM_CATEGORY_DANGEROUS_CONTENT",
            ];
            let settings: Vec<Value> = categories
                .iter()
                .map(|c| json!({ "category": c, "threshold": threshold }))
                .collect();
            body.insert("safetySettings".to_string(), Value::Array(settings));
        }

        Ok(Value::Object(body))
    }
}

fn encode_gemini_message(
    msg: &Message,
    unsigned_sentinel: &str,
    separate_system_update_after_fn_resp: bool,
) -> Vec<Value> {
    let mut out = Vec::new();
    match msg.role {
        Role::Assistant => {
            let mut parts = Vec::new();
            let mut attached_sentinel = false;
            for part in &msg.parts {
                match part {
                    ContentPart::Text { text, .. } => {
                        parts.push(json!({ "text": text }));
                    }
                    ContentPart::Reasoning {
                        text, signature, ..
                    } => {
                        let mut p = Map::new();
                        p.insert("thought".to_string(), json!(true));
                        p.insert("text".to_string(), json!(text));
                        if let Some(sig) = signature {
                            p.insert("thoughtSignature".to_string(), json!(sig));
                        }
                        parts.push(Value::Object(p));
                    }
                    ContentPart::ToolCall {
                        name,
                        arguments,
                        signature,
                        ..
                    } => {
                        let mut p = Map::new();
                        p.insert(
                            "functionCall".to_string(),
                            json!({ "name": name, "args": arguments }),
                        );
                        if let Some(sig) = signature {
                            p.insert("thoughtSignature".to_string(), json!(sig));
                        } else if !attached_sentinel {
                            p.insert("thoughtSignature".to_string(), json!(unsigned_sentinel));
                            attached_sentinel = true;
                        }
                        parts.push(Value::Object(p));
                    }
                    _ => {}
                }
            }
            if !parts.is_empty() {
                out.push(json!({ "role": "model", "parts": parts }));
            }
        }
        Role::Tool => {
            let mut fn_parts = Vec::new();
            let mut trailing_updates = Vec::new();
            for part in &msg.parts {
                match part {
                    ContentPart::ToolResult {
                        name,
                        content,
                        is_error,
                        ..
                    } => {
                        let text = content
                            .iter()
                            .filter_map(|b| match b {
                                ToolResultBlock::Text { text } => Some(text.as_str()),
                                ToolResultBlock::Media { .. } => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        fn_parts.push(json!({
                            "functionResponse": {
                                "name": name,
                                "response": { "output": text, "is_error": is_error }
                            }
                        }));
                    }
                    ContentPart::Text { text, .. } => {
                        if separate_system_update_after_fn_resp && text.contains("<system-update>")
                        {
                            trailing_updates.push(json!({ "text": text }));
                        } else {
                            fn_parts.push(json!({ "text": text }));
                        }
                    }
                    _ => {}
                }
            }
            if !fn_parts.is_empty() {
                out.push(json!({ "role": "user", "parts": fn_parts }));
            }
            if !trailing_updates.is_empty() {
                out.push(json!({ "role": "user", "parts": trailing_updates }));
            }
        }
        _ => {
            let mut parts = Vec::new();
            for part in &msg.parts {
                match part {
                    ContentPart::Text { text, .. } => parts.push(json!({ "text": text })),
                    ContentPart::Media {
                        mime_type,
                        base64_data,
                        ..
                    } => {
                        parts.push(json!({
                            "inlineData": { "mimeType": mime_type, "data": base64_data }
                        }));
                    }
                    ContentPart::Compaction {
                        summary_xml: Some(s),
                        ..
                    } => parts.push(json!({ "text": s })),
                    _ => {}
                }
            }
            if !parts.is_empty() {
                out.push(json!({ "role": "user", "parts": parts }));
            }
        }
    }
    out
}

#[derive(Default)]
pub struct GeminiSseDecoder {
    in_text: bool,
    in_reasoning: bool,
    last_thought_signature: Option<String>,
    usage: Usage,
    finish_reason: Option<FinishReason>,
    saw_finish: bool,
}

impl GeminiSseDecoder {
    pub fn push_data(&mut self, data: &str) -> Vec<Result<LLMEvent, AiError>> {
        let mut out = Vec::new();
        let val: Value = match serde_json::from_str(data.trim()) {
            Ok(v) => v,
            Err(_) => return out,
        };

        if let Some(u) = val.get("usageMetadata") {
            let prompt = u
                .get("promptTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cached = u
                .get("cachedContentTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let candidates = u
                .get("candidatesTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let thoughts = u
                .get("thoughtsTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            self.usage = Usage {
                input_tokens: prompt,
                non_cached_input_tokens: prompt.saturating_sub(cached),
                cache_read_input_tokens: cached,
                cache_write_input_tokens: 0,
                output_tokens: candidates.saturating_add(thoughts),
                reasoning_tokens: thoughts,
                provider_reported_cost_usd: None,
            };
        }

        if let Some(cand) = val
            .get("candidates")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
        {
            if let Some(parts) = cand
                .get("content")
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
            {
                for part in parts {
                    let sig = part
                        .get("thoughtSignature")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    if sig.is_some() {
                        self.last_thought_signature = sig.clone();
                    }

                    if part
                        .get("thought")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        && let Some(t) = part.get("text").and_then(|v| v.as_str())
                    {
                        if !self.in_reasoning {
                            self.in_reasoning = true;
                            out.push(Ok(LLMEvent::ReasoningStart));
                        }
                        out.push(Ok(LLMEvent::ReasoningDelta {
                            delta: t.to_string(),
                        }));
                        continue;
                    }

                    if let Some(fc) = part.get("functionCall") {
                        self.close_open_text_or_reasoning(&mut out);
                        let name = fc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let args = fc
                            .get("args")
                            .cloned()
                            .unwrap_or_else(|| Value::Object(Map::new()));
                        let id = format!("call_{}", Ulid::new());
                        out.push(Ok(LLMEvent::ToolCall {
                            id,
                            name,
                            arguments: args,
                            provider_executed: false,
                            signature: sig,
                        }));
                        self.finish_reason = Some(FinishReason::ToolCalls);
                        continue;
                    }

                    if let Some(t) = part.get("text").and_then(|v| v.as_str())
                        && !t.is_empty()
                    {
                        if self.in_reasoning {
                            self.in_reasoning = false;
                            out.push(Ok(LLMEvent::ReasoningEnd {
                                signature: self.last_thought_signature.take(),
                                redacted: None,
                            }));
                        }
                        if !self.in_text {
                            self.in_text = true;
                            out.push(Ok(LLMEvent::TextStart));
                        }
                        out.push(Ok(LLMEvent::TextDelta {
                            delta: t.to_string(),
                        }));
                    }
                }
            }

            if let Some(fr) = cand.get("finishReason").and_then(|v| v.as_str()) {
                self.saw_finish = true;
                self.close_open_text_or_reasoning(&mut out);
                let reason = match fr {
                    "STOP" => {
                        if self.finish_reason == Some(FinishReason::ToolCalls) {
                            FinishReason::ToolCalls
                        } else {
                            FinishReason::Stop
                        }
                    }
                    "MAX_TOKENS" => FinishReason::Length,
                    "SAFETY" | "RECITATION" | "PROHIBITED_CONTENT" => FinishReason::ContentFilter,
                    _ => FinishReason::Unknown,
                };
                self.finish_reason = Some(reason);
                out.push(Ok(LLMEvent::Finish {
                    reason,
                    usage: self.usage.clone(),
                }));
            }
        }

        out
    }

    pub fn on_halt(&mut self) -> Vec<Result<LLMEvent, AiError>> {
        if self.saw_finish {
            return Vec::new();
        }
        vec![Err(AiError::IncompleteStream {
            message: "Gemini SSE stream terminated before finishReason".to_string(),
        })]
    }

    fn close_open_text_or_reasoning(&mut self, out: &mut Vec<Result<LLMEvent, AiError>>) {
        if self.in_reasoning {
            self.in_reasoning = false;
            out.push(Ok(LLMEvent::ReasoningEnd {
                signature: self.last_thought_signature.take(),
                redacted: None,
            }));
        }
        if self.in_text {
            self.in_text = false;
            out.push(Ok(LLMEvent::TextEnd));
        }
    }
}

#[async_trait]
impl WireProtocol for GeminiGenerateContentProtocol {
    fn protocol_id(&self) -> &str {
        "gemini_generate_content"
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, AiError> {
        let payload = Self::build_payload(&request).map_err(|e| AiError::InvalidRequest {
            message: e.to_string(),
        })?;
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
            request.route.base_url.trim_end_matches('/'),
            request.model.model_id
        );
        let mut req_builder = self.http.post(&url).json(&payload);
        if let Some(key) = saint_harness_schema::resolve_api_key(&request.route.api_key_env) {
            req_builder = req_builder.header("x-goog-api-key", key);
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
                GeminiSseDecoder::default(),
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

    #[test]
    fn encodes_unsigned_sentinel_fnresp_split_and_safety_settings() {
        let profile = embedded_profile("gemini-flagship").unwrap();
        let routes = embedded_routes().unwrap();
        let route = routes.routes.get("google_ai_studio").unwrap().clone();
        let req = LlmRequest {
            system: vec!["sys".into()],
            messages: vec![
                Message {
                    id: "m1".into(),
                    parent_msg_id: None,
                    role: Role::Assistant,
                    parts: vec![
                        ContentPart::ToolCall {
                            id: "c1".into(),
                            name: "read".into(),
                            arguments: json!({"path": "a.rs"}),
                            provider_executed: false,
                            signature: None,
                        },
                        ContentPart::ToolCall {
                            id: "c2".into(),
                            name: "read".into(),
                            arguments: json!({"path": "b.rs"}),
                            provider_executed: false,
                            signature: None,
                        },
                    ],
                    model_provider: Some("qwen".into()),
                    model_id: None,
                },
                Message {
                    id: "m2".into(),
                    parent_msg_id: Some("m1".into()),
                    role: Role::Tool,
                    parts: vec![
                        ContentPart::ToolResult {
                            call_id: "c1".into(),
                            name: "read".into(),
                            content: vec![ToolResultBlock::Text { text: "ok".into() }],
                            is_error: false,
                        },
                        ContentPart::Text {
                            text: "<system-update>mode=plan</system-update>".into(),
                            provider_meta: None,
                        },
                    ],
                    model_provider: None,
                    model_id: None,
                },
            ],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            max_output_tokens: 4096,
            model: ResolvedModel {
                provider: "google".into(),
                model_id: "gemini-test".into(),
                display_name: "Gemini".into(),
                native_window: 1_000_000,
                effective_window: 800_000,
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
        let payload = GeminiGenerateContentProtocol::build_payload(&req).unwrap();
        // Sentinel attached ONLY to first unsigned tool call
        assert_eq!(
            payload["contents"][0]["parts"][0]["thoughtSignature"],
            json!("skip_thought_signature_validator")
        );
        assert!(
            payload["contents"][0]["parts"][1]
                .get("thoughtSignature")
                .is_none()
        );
        // <system-update> split into a fresh user turn after functionResponse
        assert_eq!(payload["contents"].as_array().unwrap().len(), 3);
        assert_eq!(payload["contents"][2]["role"], json!("user"));
        assert_eq!(
            payload["safetySettings"][0]["threshold"],
            json!("BLOCK_NONE")
        );
        assert!(
            !serde_json::to_string(&payload)
                .unwrap()
                .contains("cache_control")
        );
    }

    #[test]
    fn cassette_thought_signature_and_saturating_usage() {
        let mut dec = GeminiSseDecoder::default();
        let mut events = Vec::new();
        events.extend(dec.push_data(
            r#"{"candidates":[{"content":{"parts":[{"thought":true,"text":"hmm","thoughtSignature":"sig_g1"},{"functionCall":{"name":"read","args":{"path":"x.rs"}},"thoughtSignature":"sig_g2"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":500,"cachedContentTokenCount":700,"candidatesTokenCount":20,"thoughtsTokenCount":10}}"#,
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            Ok(LLMEvent::ToolCall { signature: Some(s), .. }) if s == "sig_g2"
        )));
        let finish = events.iter().find_map(|e| match e {
            Ok(LLMEvent::Finish { usage, reason }) => Some((*reason, usage.clone())),
            _ => None,
        });
        let (reason, usage) = finish.expect("finish");
        assert_eq!(reason, FinishReason::ToolCalls);
        assert_eq!(usage.non_cached_input_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 700);
    }
}
