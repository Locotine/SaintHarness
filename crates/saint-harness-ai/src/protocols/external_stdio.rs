use async_trait::async_trait;
use futures::stream;
use saint_harness_schema::{AiError, LLMEvent, LlmRequest, LlmStream, WireProtocol};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// Escape-hatch wire protocol invoking an external command speaking newline-delimited JSON on stdin/stdout.
pub struct ExternalStdioProtocol {
    pub command: String,
    pub args: Vec<String>,
}

impl ExternalStdioProtocol {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

#[async_trait]
impl WireProtocol for ExternalStdioProtocol {
    fn protocol_id(&self) -> &str {
        "external_stdio"
    }

    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, AiError> {
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AiError::InvalidRequest {
                message: format!("failed to spawn external stdio protocol: {e}"),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            let line = serde_json::to_string(&request).map_err(|e| AiError::InvalidRequest {
                message: e.to_string(),
            })?;
            stdin
                .write_all(format!("{line}\n").as_bytes())
                .await
                .map_err(|e| AiError::InvalidRequest {
                    message: e.to_string(),
                })?;
            let _ = stdin.shutdown().await;
        }

        let stdout = child.stdout.take().ok_or_else(|| AiError::InvalidRequest {
            message: "external stdio protocol missing stdout".to_string(),
        })?;

        let lines = BufReader::new(stdout).lines();
        let out = stream::unfold((lines, child), |(mut lines, mut child)| async move {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return Some((Ok(LLMEvent::StepStart), (lines, child)));
                    }
                    let parsed = serde_json::from_str::<LLMEvent>(trimmed).map_err(|e| {
                        AiError::InvalidRequest {
                            message: format!("invalid external_stdio event JSON: {e}"),
                        }
                    });
                    Some((parsed, (lines, child)))
                }
                Ok(None) => {
                    let _ = child.wait().await;
                    None
                }
                Err(e) => {
                    let _ = child.kill().await;
                    Some((
                        Err(AiError::IncompleteStream {
                            message: e.to_string(),
                        }),
                        (lines, child),
                    ))
                }
            }
        });

        Ok(Box::pin(out))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use saint_harness_schema::{
        FinishReason, ResolvedModel, ToolChoice, Usage, embedded_profile, embedded_routes,
    };
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn external_stdio_protocol_streams_json_lines() {
        let ev1 = serde_json::to_string(&LLMEvent::TextDelta {
            delta: "hello".into(),
        })
        .unwrap();
        let ev2 = serde_json::to_string(&LLMEvent::Finish {
            reason: FinishReason::Stop,
            usage: Usage::default(),
        })
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let fixture = tmp.path().join("events.jsonl");
        std::fs::write(&fixture, format!("{ev1}\n{ev2}\n")).unwrap();
        #[cfg(unix)]
        let proto = ExternalStdioProtocol::new("cat", vec![fixture.to_string_lossy().into_owned()]);
        #[cfg(windows)]
        let proto = ExternalStdioProtocol::new(
            "cmd.exe",
            vec![
                "/C".into(),
                "type".into(),
                fixture.to_string_lossy().into_owned(),
            ],
        );
        let profile = embedded_profile("qwen-3.8-27b").unwrap();
        let routes = embedded_routes().unwrap();
        let route = routes.routes.get("qwen_local_sglang").unwrap().clone();
        let req = LlmRequest {
            system: vec![],
            messages: vec![],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
            max_output_tokens: 1024,
            model: ResolvedModel {
                provider: "qwen".into(),
                model_id: "qwen3.8-27b".into(),
                display_name: "Qwen".into(),
                native_window: 262_144,
                effective_window: 110_000,
                max_output_tokens: 16_384,
                supports_adaptive_thinking: false,
                supports_vision: false,
                resolution_source: "test".into(),
            },
            profile,
            route,
            session_affinity: "s1".into(),
            smol_mode: false,
            extra_headers: BTreeMap::new(),
        };
        let mut s = proto.stream(req).await.unwrap();
        let mut items = Vec::new();
        while let Some(item) = s.next().await {
            items.push(item.unwrap());
        }
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], LLMEvent::TextDelta { ref delta } if delta == "hello"));
        assert!(matches!(
            items[1],
            LLMEvent::Finish {
                reason: FinishReason::Stop,
                ..
            }
        ));
    }
}
