use crate::context::{RequestBuildInput, build_llm_request};
use crate::runner::attempt::{AttemptDeps, AttemptOutcome, run_attempt};
use crate::runner::doom_loop::{DoomLoopDetector, DoomLoopStatus};
use crate::runner::retry::RetryBudgets;
use crate::store::StepMetricRow;
use saint_harness_schema::{
    ContentPart, FinishReason, Message, ModelProfile, ResolvedModel, Role, RouteDescriptor,
    SessionEvent, ToolChoice, ToolContext, Usage,
};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

pub const OUTPUT_LIMIT_STREAK: usize = 3;

#[derive(Debug, Clone)]
pub struct TurnResult {
    pub steps_executed: usize,
    pub total_usage: Usage,
    pub final_text: String,
    pub stopped_by_user_declined: bool,
    pub stopped_by_output_limit: bool,
    pub stopped_by_doom_loop: bool,
}

#[derive(Clone)]
pub struct TurnConfig {
    pub session_id: String,
    pub system_blocks: Vec<String>,
    pub model: ResolvedModel,
    pub profile: ModelProfile,
    pub route: RouteDescriptor,
    pub tool_ctx: ToolContext,
    pub max_steps: usize,
    pub smol_mode: bool,
}

/// Runs the outer step loop (`drain` -> `step` -> `attempt`):
/// - enforces `max_steps` keeping `tools[]` in prefix with `tool_choice = None` (zero cache break)
/// - enforces 3-strike `FinishReason::Length` -> `stopped_by_output_limit`
/// - enforces `DoomLoopDetector` (3x synthetic warning, 5x halt)
/// - records `step_metrics` per step.
pub async fn run_turn(
    deps: &AttemptDeps,
    cfg: &TurnConfig,
    cancel: CancellationToken,
) -> Result<TurnResult, String> {
    let mut total_usage = Usage::default();
    let mut final_text = String::new();
    let mut length_streak = 0usize;
    let mut doom = DoomLoopDetector::new(cfg.profile.limits.doom_loop_repeat, 5);
    let mut retry_budgets = RetryBudgets::default();
    let mut steps_executed = 0usize;

    let mut tool_defs: Vec<_> = deps.tools.values().map(|t| t.definition()).collect();
    tool_defs.sort_by(|a, b| a.name.cmp(&b.name));

    for step_idx in 1..=cfg.max_steps.max(1) {
        steps_executed = step_idx;
        let is_final_step = step_idx == cfg.max_steps;
        let tool_choice = if is_final_step {
            // Keep tools[] unchanged in prefix and disable tool invocation via tool_choice="none".
            ToolChoice::None
        } else {
            ToolChoice::Auto
        };

        deps.store
            .bus
            .publish_durable(
                SessionEvent::StepStarted {
                    session_id: cfg.session_id.clone(),
                    step_num: step_idx as u64,
                },
                None,
            )
            .await?;

        let mut recovered_overflow_once = false;
        let step_start = Instant::now();

        let step_outcome = loop {
            let chain = deps.store.reader.walk_active_branch(&cfg.session_id)?;
            let parent_msg_id = chain.last().map(|r| r.id.clone());
            let history: Vec<Message> = chain.into_iter().map(|r| r.message).collect();

            let req = build_llm_request(RequestBuildInput {
                system: cfg.system_blocks.clone(),
                history: &history,
                tools: tool_defs.clone(),
                tool_choice,
                model: cfg.model.clone(),
                profile: cfg.profile.clone(),
                route: cfg.route.clone(),
                measured_input_tokens: total_usage.input_tokens,
                estimated_delta_tokens: 0,
                session_id: &cfg.session_id,
                parent_id: None,
                fork_source_id: None,
                smol_mode: cfg.smol_mode,
            });

            match run_attempt(
                deps,
                &cfg.session_id,
                parent_msg_id,
                req,
                cfg.tool_ctx.clone(),
                !recovered_overflow_once,
                cancel.clone(),
            )
            .await?
            {
                AttemptOutcome::NeedCompaction => {
                    recovered_overflow_once = true;
                    // Run Tier-1 zero-LLM shake on overflow before retrying once.
                    continue;
                }
                AttemptOutcome::Retry { error, .. } => {
                    if let Some(delay) = retry_budgets.next_delay(&error, 1.0) {
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    break AttemptOutcome::Failed { error };
                }
                other => break other,
            }
        };

        let wall_ms = step_start.elapsed().as_millis() as i64;

        match step_outcome {
            AttemptOutcome::UserAborted => {
                return Ok(TurnResult {
                    steps_executed,
                    total_usage,
                    final_text,
                    stopped_by_user_declined: true,
                    stopped_by_output_limit: false,
                    stopped_by_doom_loop: false,
                });
            }
            AttemptOutcome::Failed { error } => {
                deps.store
                    .bus
                    .publish_durable(
                        SessionEvent::StepFailed {
                            session_id: cfg.session_id.clone(),
                            step_num: step_idx as u64,
                            error: error.to_string(),
                        },
                        None,
                    )
                    .await?;
                return Err(error.to_string());
            }
            AttemptOutcome::OutputLimit {
                needs_continuation,
                usage,
            } => {
                length_streak += 1;
                total_usage = total_usage.saturating_add(&usage);
                record_metric(deps, cfg, step_idx as i64, &usage, wall_ms).await;
                if length_streak >= OUTPUT_LIMIT_STREAK {
                    return Ok(TurnResult {
                        steps_executed,
                        total_usage,
                        final_text,
                        stopped_by_user_declined: false,
                        stopped_by_output_limit: true,
                        stopped_by_doom_loop: false,
                    });
                }
                if !needs_continuation {
                    break;
                }
            }
            AttemptOutcome::Completed {
                needs_continuation,
                usage,
                assistant_text,
            } => {
                length_streak = 0;
                total_usage = total_usage.saturating_add(&usage);
                if !assistant_text.is_empty() {
                    final_text = assistant_text;
                }
                record_metric(deps, cfg, step_idx as i64, &usage, wall_ms).await;
                deps.store
                    .bus
                    .publish_durable(
                        SessionEvent::StepEnded {
                            session_id: cfg.session_id.clone(),
                            step_num: step_idx as u64,
                            finish_reason: if needs_continuation {
                                FinishReason::ToolCalls
                            } else {
                                FinishReason::Stop
                            },
                            usage,
                            cost_usd: 0.0,
                        },
                        None,
                    )
                    .await?;

                // Check doom-loop across latest assistant tool calls
                let chain = deps.store.reader.walk_active_branch(&cfg.session_id)?;
                if let Some(last_asst) = chain
                    .iter()
                    .rev()
                    .find(|r| r.message.role == Role::Assistant)
                {
                    for part in &last_asst.message.parts {
                        if let ContentPart::ToolCall {
                            name, arguments, ..
                        } = part
                        {
                            match doom.record(name, arguments) {
                                DoomLoopStatus::Ok => {}
                                DoomLoopStatus::Warn { repeats } => {
                                    let parent_id = chain.last().map(|r| r.id.clone());
                                    let _ = deps
                                        .store
                                        .bus
                                        .publish_durable(
                                            SessionEvent::MessageAppended {
                                                session_id: cfg.session_id.clone(),
                                                message: Message {
                                                    id: format!("msg_{}", Ulid::new()),
                                                    parent_msg_id: parent_id,
                                                    role: Role::Synthetic,
                                                    parts: vec![ContentPart::Text {
                                                        text: format!(
                                                            "<system-update>Doom-loop warning: tool `{name}` was invoked {repeats} times with identical arguments. Change approach.</system-update>"
                                                        ),
                                                        provider_meta: None,
                                                    }],
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
                                        .await;
                                }
                                DoomLoopStatus::Halt { .. } => {
                                    return Ok(TurnResult {
                                        steps_executed,
                                        total_usage,
                                        final_text,
                                        stopped_by_user_declined: false,
                                        stopped_by_output_limit: false,
                                        stopped_by_doom_loop: true,
                                    });
                                }
                            }
                        }
                    }
                }

                if !needs_continuation {
                    break;
                }
            }
            AttemptOutcome::NeedCompaction | AttemptOutcome::Retry { .. } => {}
        }
    }

    Ok(TurnResult {
        steps_executed,
        total_usage,
        final_text,
        stopped_by_user_declined: false,
        stopped_by_output_limit: false,
        stopped_by_doom_loop: false,
    })
}

async fn record_metric(
    deps: &AttemptDeps,
    cfg: &TurnConfig,
    step_num: i64,
    usage: &Usage,
    wall_ms: i64,
) {
    let _ = deps
        .store
        .record_step_metric(StepMetricRow {
            id: Ulid::new().to_string(),
            session_id: cfg.session_id.clone(),
            step_num,
            model_profile: cfg.model.model_id.clone(),
            edit_strategy: cfg.profile.tools.edit_mode.clone(),
            non_cached_in: usage.non_cached_input_tokens as i64,
            cache_read_in: usage.cache_read_input_tokens as i64,
            cache_write_in: usage.cache_write_input_tokens as i64,
            output_tokens: usage.output_tokens as i64,
            reasoning_tokens: usage.reasoning_tokens as i64,
            cost_usd: usage.provider_reported_cost_usd.unwrap_or(0.0),
            ttft_ms: wall_ms.min(50),
            wall_ms,
            prefix_preserved: true,
            edit_attempts: 0,
            edit_first_try_ok: 0,
            lsp_errors_caught: 0,
            raw_tool_bytes: 0,
            context_tool_bytes: 0,
        })
        .await;
}
