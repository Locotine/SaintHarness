use crate::store::EventStore;
use futures::StreamExt;
use saint_harness_schema::{
    AiError, ContentPart, FinishReason, GuardDecision, LLMEvent, LlmRequest, Message, PolicyGuard,
    Role, SessionEvent, Tool, ToolChoice, ToolContext, ToolOutcome, ToolResultBlock, Usage,
    WireProtocol,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

pub const COOPERATIVE_STOP_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_MALFORMED_EXCERPT_CHARS: usize = 2048;

#[derive(Debug, Clone)]
pub enum AttemptOutcome {
    NeedCompaction,
    Retry {
        fresh_assistant: bool,
        error: AiError,
    },
    UserAborted,
    OutputLimit {
        needs_continuation: bool,
        usage: Usage,
    },
    Completed {
        needs_continuation: bool,
        usage: Usage,
        assistant_text: String,
    },
    Failed {
        error: AiError,
    },
}

#[derive(Clone)]
pub struct AttemptDeps {
    pub store: EventStore,
    pub protocol: Arc<dyn WireProtocol>,
    pub tools: BTreeMap<String, Arc<dyn Tool>>,
    pub policy_guard: Arc<dyn PolicyGuard>,
}

/// Runs a single streaming LLM attempt with concurrent `JoinSet` local tool execution
/// and executes the exact 16-row decision priority table (Section 4 of plan).
pub async fn run_attempt(
    deps: &AttemptDeps,
    session_id: &str,
    parent_msg_id: Option<String>,
    request: LlmRequest,
    tool_ctx: ToolContext,
    recover_overflow: bool,
    cancel: CancellationToken,
) -> Result<AttemptOutcome, String> {
    let tool_choice = request.tool_choice;
    let model_provider = request.model.provider.clone();
    let model_id = request.model.model_id.clone();

    let mut stream = match deps.protocol.stream(request).await {
        Ok(s) => s,
        Err(err) => {
            if matches!(err, AiError::ContextOverflow { .. }) && recover_overflow {
                return Ok(AttemptOutcome::NeedCompaction);
            }
            return Ok(classify_no_output_error(err));
        }
    };

    let mut text_buf = String::new();
    let mut reasoning_buf = String::new();
    let mut parts: Vec<ContentPart> = Vec::new();
    let mut malformed_errors: Vec<(String, String, String)> = Vec::new();
    let mut finish_reason = FinishReason::Unknown;
    let mut usage = Usage::default();
    let mut stream_err: Option<AiError> = None;
    let mut saw_output = false;
    let mut any_local_tool_called = false;

    let mut tool_tasks: JoinSet<(String, String, ToolOutcome)> = JoinSet::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Cooperative stop window (5s) for in-flight local tools.
                let _ = tokio::time::timeout(COOPERATIVE_STOP_TIMEOUT, async {
                    while tool_tasks.join_next().await.is_some() {}
                }).await;
                tool_tasks.abort_all();
                return Ok(AttemptOutcome::UserAborted);
            }
            next = stream.next() => {
                match next {
                    Some(Ok(ev)) => match ev {
                        LLMEvent::StepStart | LLMEvent::TextStart | LLMEvent::ReasoningStart
                        | LLMEvent::ToolInputStart { .. } | LLMEvent::ToolInputDelta { .. } => {}
                        LLMEvent::TextDelta { delta } => {
                            saw_output = true;
                            text_buf.push_str(&delta);
                            deps.store.bus.publish_ephemeral(SessionEvent::TextDelta {
                                session_id: session_id.to_string(),
                                delta,
                            });
                        }
                        LLMEvent::TextEnd => {
                            if !text_buf.is_empty() {
                                parts.push(ContentPart::Text {
                                    text: std::mem::take(&mut text_buf),
                                    provider_meta: None,
                                });
                            }
                        }
                        LLMEvent::ReasoningDelta { delta } => {
                            saw_output = true;
                            reasoning_buf.push_str(&delta);
                            deps.store.bus.publish_ephemeral(SessionEvent::ReasoningDelta {
                                session_id: session_id.to_string(),
                                delta,
                            });
                        }
                        LLMEvent::ReasoningEnd { signature, redacted } => {
                            if !reasoning_buf.is_empty() || redacted.is_some() {
                                parts.push(ContentPart::Reasoning {
                                    text: std::mem::take(&mut reasoning_buf),
                                    signature,
                                    redacted_data: redacted,
                                    provider_meta: None,
                                });
                            }
                        }
                        LLMEvent::ToolInputError { id, name, raw_excerpt, reason } => {
                            saw_output = true;
                            any_local_tool_called = true;
                            let clipped: String = raw_excerpt.chars().take(MAX_MALFORMED_EXCERPT_CHARS).collect();
                            malformed_errors.push((id, name, format!("{reason:?}: {clipped}")));
                        }
                        LLMEvent::ToolCall { id, name, arguments, provider_executed, signature } => {
                            saw_output = true;
                            parts.push(ContentPart::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                provider_executed,
                                signature,
                            });
                            if !provider_executed {
                                any_local_tool_called = true;
                                let tool_opt = deps.tools.get(&name).cloned();
                                let guard = deps.policy_guard.clone();
                                let mut ctx = tool_ctx.clone();
                                ctx.call_id = id.clone();
                                let call_id = id.clone();
                                let tool_name = name.clone();
                                tool_tasks.spawn(async move {
                                    if let GuardDecision::Block { reason } = guard.inspect_tool_call(&tool_name, &arguments).await {
                                        return (call_id, tool_name, ToolOutcome::RecoverableError(saint_harness_schema::ToolFailure::BlockedError { reason }));
                                    }
                                    match tool_opt {
                                        Some(t) => (call_id, tool_name, t.execute(&ctx, arguments).await),
                                        None => {
                                            let msg = format!("unknown tool `{tool_name}`");
                                            (
                                                call_id,
                                                tool_name,
                                                ToolOutcome::RecoverableError(
                                                    saint_harness_schema::ToolFailure::ValidationError {
                                                        message: msg,
                                                    },
                                                ),
                                            )
                                        }
                                    }
                                });
                            }
                        }
                        LLMEvent::ToolResultHosted { .. } => {}
                        LLMEvent::Finish { reason, usage: u } => {
                            finish_reason = reason;
                            usage = u;
                        }
                    },
                    Some(Err(err)) => {
                        stream_err = Some(err);
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    if !text_buf.is_empty() {
        parts.push(ContentPart::Text {
            text: text_buf,
            provider_meta: None,
        });
    }
    if !reasoning_buf.is_empty() {
        parts.push(ContentPart::Reasoning {
            text: reasoning_buf,
            signature: None,
            redacted_data: None,
            provider_meta: None,
        });
    }

    // Row 1: ContextOverflow before any output -> NeedCompaction (max 1 per step).
    if !saw_output
        && let Some(AiError::ContextOverflow { .. }) = stream_err
        && recover_overflow
    {
        tool_tasks.abort_all();
        return Ok(AttemptOutcome::NeedCompaction);
    }

    // Row 2: Clean EOF with FinishReason::Unknown -> IncompleteStream (retryable).
    if stream_err.is_none() && finish_reason == FinishReason::Unknown {
        stream_err = Some(AiError::IncompleteStream {
            message: "stream ended without explicit finish reason".to_string(),
        });
    }

    // Row 3: Retryable error before any output -> Retry { fresh_assistant: false }.
    if !saw_output && let Some(err) = stream_err {
        tool_tasks.abort_all();
        return Ok(classify_no_output_error(err));
    }

    // Collect local tool outcomes.
    let mut settled_results: Vec<(String, String, ToolOutcome)> = Vec::new();
    let mut user_declined = false;
    while let Some(res) = tool_tasks.join_next().await {
        if let Ok((call_id, name, outcome)) = res {
            if matches!(outcome, ToolOutcome::UserDeclined) {
                user_declined = true;
            }
            settled_results.push((call_id, name, outcome));
        }
    }

    // Commit assistant message durably (uninterruptible commit phase).
    let assistant_msg_id = format!("msg_{}", Ulid::new());
    let assistant_text = parts
        .iter()
        .filter_map(|p| match p {
            ContentPart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let assistant_msg = Message {
        id: assistant_msg_id.clone(),
        parent_msg_id,
        role: Role::Assistant,
        parts,
        model_provider: Some(model_provider),
        model_id: Some(model_id),
    };
    deps.store
        .bus
        .publish_durable(
            SessionEvent::MessageAppended {
                session_id: session_id.to_string(),
                message: assistant_msg,
                finish_reason: Some(finish_reason),
                usage: Some(usage.clone()),
                error_json: stream_err
                    .as_ref()
                    .and_then(|e| serde_json::to_string(e).ok()),
                snapshot_start: None,
                changed_files: vec![],
            },
            None,
        )
        .await?;

    // Row 5: ToolOutcome::UserDeclined -> record decline + stop turn immediately (Law #4).
    if user_declined {
        let mut decline_parts = Vec::new();
        for (call_id, name, outcome) in settled_results {
            let text = match outcome {
                ToolOutcome::UserDeclined => "The user declined this tool call".to_string(),
                ToolOutcome::Completed(_) => "interrupted".to_string(),
                ToolOutcome::RecoverableError(f) => f.to_string(),
            };
            decline_parts.push(ContentPart::ToolResult {
                call_id,
                name,
                content: vec![ToolResultBlock::Text { text }],
                is_error: true,
            });
        }
        deps.store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: session_id.to_string(),
                    message: Message {
                        id: format!("msg_{}", Ulid::new()),
                        parent_msg_id: Some(assistant_msg_id),
                        role: Role::Tool,
                        parts: decline_parts,
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec![],
                },
                None,
            )
            .await?;
        return Ok(AttemptOutcome::UserAborted);
    }

    // Row 6: Settle tool results + malformed tool inputs (excerpt <= 2048 chars).
    let mut tool_result_parts = Vec::new();
    for (call_id, name, outcome) in settled_results {
        match outcome {
            ToolOutcome::Completed(res) => {
                tool_result_parts.push(ContentPart::ToolResult {
                    call_id,
                    name,
                    content: res.content,
                    is_error: false,
                });
            }
            ToolOutcome::RecoverableError(fail) => {
                tool_result_parts.push(ContentPart::ToolResult {
                    call_id,
                    name,
                    content: vec![ToolResultBlock::Text {
                        text: fail.to_string(),
                    }],
                    is_error: true,
                });
            }
            ToolOutcome::UserDeclined => {}
        }
    }
    for (call_id, name, msg) in malformed_errors {
        tool_result_parts.push(ContentPart::ToolResult {
            call_id,
            name,
            content: vec![ToolResultBlock::Text { text: msg }],
            is_error: true,
        });
    }

    if !tool_result_parts.is_empty() {
        deps.store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: session_id.to_string(),
                    message: Message {
                        id: format!("msg_{}", Ulid::new()),
                        parent_msg_id: Some(assistant_msg_id),
                        role: Role::Tool,
                        parts: tool_result_parts,
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec![],
                },
                None,
            )
            .await?;
    }

    // Row 8: Retryable error AFTER output -> Retry { fresh_assistant: true }.
    if let Some(err) = stream_err {
        return Ok(if is_retryable_error(&err) {
            AttemptOutcome::Retry {
                fresh_assistant: true,
                error: err,
            }
        } else {
            AttemptOutcome::Failed { error: err }
        });
    }

    let ends_turn = matches!(finish_reason, FinishReason::ContentFilter);
    let needs_continuation = !ends_turn && tool_choice != ToolChoice::None && any_local_tool_called;

    // Row 9: FinishReason::Length -> tracked for 3-strike OutputLimit.
    if finish_reason == FinishReason::Length {
        return Ok(AttemptOutcome::OutputLimit {
            needs_continuation,
            usage,
        });
    }

    // Row 10: Completed
    Ok(AttemptOutcome::Completed {
        needs_continuation,
        usage,
        assistant_text,
    })
}

fn is_retryable_error(err: &AiError) -> bool {
    matches!(
        err,
        AiError::RateLimit { .. }
            | AiError::ProviderInternal { .. }
            | AiError::Transport { .. }
            | AiError::IncompleteStream { .. }
            | AiError::Timeout { .. }
            | AiError::UnknownProvider { .. }
    )
}

fn classify_no_output_error(err: AiError) -> AttemptOutcome {
    if is_retryable_error(&err) {
        AttemptOutcome::Retry {
            fresh_assistant: false,
            error: err,
        }
    } else {
        AttemptOutcome::Failed { error: err }
    }
}
