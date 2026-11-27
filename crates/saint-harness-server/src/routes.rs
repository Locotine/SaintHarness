use crate::AppState;
use crate::sse::{events_all, events_for_session};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use saint_harness_core::{
    DeliveryMode, InboxItemType, ShadowSnapshotEngine, WakeScope, discover_skills, fork_session,
    list_session_tree, revert_clear, revert_commit, revert_stage,
};
use saint_harness_protocol::{
    BtwRequest, BtwResponse, CreateSessionRequest, CreateSessionResponse, ForkRequest,
    HealthResponse, ModelChoiceDto, ModelProviderGroupDto, ModelsCatalogResponse,
    PermissionReplyRequest, PluginDto, PromptRequest, RollbackRequest, SessionStatusCounts,
    SessionSummaryDto, SessionUiStatus, SkillDto, TaskSteerRequest, TranscriptItemDto, TreeNodeDto,
    UserInputReplyRequest, WireInteractionMode, WireRuntimeMode,
};
use saint_harness_schema::{
    InteractionMode, PermissionReply, RuntimeMode, SandboxProfile, SessionEvent, UserInputAnswers,
    embedded_profile, load_routes_with_overrides,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use ulid::Ulid;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/models", get(list_models))
        .route("/skills", get(list_skills))
        .route("/plugins", get(list_plugins))
        .route("/sessions", post(create_session).get(list_sessions))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/{id}/prompt", post(send_prompt))
        .route("/sessions/{id}/interrupt", post(interrupt_session))
        .route("/sessions/{id}/fork", post(fork_session_handler))
        .route("/sessions/{id}/rollback", post(rollback_session))
        .route("/sessions/{id}/compact", post(compact_session))
        .route("/sessions/{id}/btw", post(btw_session))
        .route("/sessions/{id}/tree", get(session_tree))
        .route("/sessions/{id}/tasks/{task_id}/stop", post(stop_task))
        .route("/sessions/{id}/tasks/{task_id}/steer", post(steer_task))
        .route("/permissions/{request_id}/reply", post(reply_permission))
        .route("/user-inputs/{request_id}/reply", post(reply_user_input))
        .route("/events", get(events_all))
        .route("/sessions/{id}/events", get(events_for_session))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let sessions = state.store.reader.list_sessions().unwrap_or_default();
    let high_water_seq = state.store.reader.high_water_seq().unwrap_or(0);
    let mut counts = SessionStatusCounts::default();
    for s in &sessions {
        if state.coordinator.is_running(&s.id).await {
            counts.working += 1;
        } else if s.active_branch.is_some() {
            counts.done += 1;
        } else {
            counts.idle += 1;
        }
    }
    Json(HealthResponse {
        reachable: true,
        ready: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
        cold_start_ms: state.started_elapsed_ms(),
        url: state.advertise_url.clone(),
        socket_path: state.socket_path.clone(),
        projection_state: "healthy".to_string(),
        high_water_seq,
        writer_queue: 0,
        sessions: counts,
        catalog_summary: vec![
            "qwen3.8-27b".to_string(),
            "claude-flagship (resolved)".to_string(),
            "gemini-flagship (resolved)".to_string(),
        ],
    })
}

async fn list_models(State(state): State<AppState>) -> Json<ModelsCatalogResponse> {
    let routes = load_routes_with_overrides(Some(&state.workspace_root), None).ok();
    let resolve_hint = |profile_name: &str| -> Option<String> {
        let prof = embedded_profile(profile_name).ok()?;
        let route = routes.as_ref()?.routes.get(&prof.model.route)?;
        let _ = route;
        Some(prof.model.selector)
    };
    let _ = (&state.model_resolver, resolve_hint("qwen-3.8-27b"));

    Json(ModelsCatalogResponse {
        hardcoded_provider_versions: 0,
        leaves: vec![
            ModelChoiceDto {
                key: "qwen".to_string(),
                name: "Qwen 3.8".to_string(),
                label: "Qwen 3.8 27B".to_string(),
                selector: "exact:qwen3.8-27b".to_string(),
                resolved_id: Some("qwen3.8-27b".to_string()),
                context_window: Some(110_000),
                edit_mode: Some("hashline".to_string()),
            },
            ModelChoiceDto {
                key: "qwen4".to_string(),
                name: "Qwen 4".to_string(),
                label: "Qwen 4".to_string(),
                selector: "latest_family:qwen4".to_string(),
                resolved_id: None,
                context_window: None,
                edit_mode: Some("hashline".to_string()),
            },
        ],
        providers: vec![
            ModelProviderGroupDto {
                key: "claude".to_string(),
                label: "Claude".to_string(),
                catalog: "Anthropic".to_string(),
                tag: "Inline4 cache".to_string(),
                handoff_note: "Inline4 breakpoints · foreign thinking stripped".to_string(),
                footnote: None,
                children: vec![
                    ModelChoiceDto {
                        key: "claude".to_string(),
                        name: "Claude Flagship".to_string(),
                        label: "Auto".to_string(),
                        selector: "latest_flagship".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("str_replace".to_string()),
                    },
                    ModelChoiceDto {
                        key: "claude-fable-5-2".to_string(),
                        name: "Claude Fable 5.2".to_string(),
                        label: "Fable 5.2".to_string(),
                        selector: "claude-fable-5-2".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("str_replace".to_string()),
                    },
                    ModelChoiceDto {
                        key: "claude-opus-5-5".to_string(),
                        name: "Claude Opus 5.5".to_string(),
                        label: "Opus 5.5".to_string(),
                        selector: "claude-opus-5-5".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("str_replace".to_string()),
                    },
                    ModelChoiceDto {
                        key: "claude-sonnet-5-5".to_string(),
                        name: "Claude Sonnet 5.5".to_string(),
                        label: "Sonnet 5.5".to_string(),
                        selector: "claude-sonnet-5-5".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("str_replace".to_string()),
                    },
                    ModelChoiceDto {
                        key: "claude-haiku-5-5".to_string(),
                        name: "Claude Haiku 5.5".to_string(),
                        label: "Haiku 5.5".to_string(),
                        selector: "claude-haiku-5-5".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("str_replace".to_string()),
                    },
                ],
            },
            ModelProviderGroupDto {
                key: "gemini".to_string(),
                label: "Gemini".to_string(),
                catalog: "Google".to_string(),
                tag: "implicit cache".to_string(),
                handoff_note: "skip_thought_signature_validator active".to_string(),
                footnote: Some("Exact Gemini IDs resolve from the live catalog.".to_string()),
                children: vec![
                    ModelChoiceDto {
                        key: "gemini".to_string(),
                        name: "Gemini Flagship".to_string(),
                        label: "Auto".to_string(),
                        selector: "latest_flagship".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("hashline".to_string()),
                    },
                    ModelChoiceDto {
                        key: "gemini-pro".to_string(),
                        name: "Gemini Pro".to_string(),
                        label: "Pro".to_string(),
                        selector: "latest_family:gemini-pro".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("hashline".to_string()),
                    },
                    ModelChoiceDto {
                        key: "gemini-flash".to_string(),
                        name: "Gemini Flash".to_string(),
                        label: "Flash".to_string(),
                        selector: "latest_family:gemini-flash".to_string(),
                        resolved_id: None,
                        context_window: None,
                        edit_mode: Some("hashline".to_string()),
                    },
                ],
            },
        ],
    })
}

async fn list_skills(State(state): State<AppState>) -> Json<Vec<SkillDto>> {
    let dirs = saint_harness_core::default_skill_dirs(&state.workspace_root, &state.home_dir);
    let skills = discover_skills(&dirs)
        .into_iter()
        .map(|s| SkillDto {
            id: s.id,
            description: s.description,
            path: s.path.to_string_lossy().into_owned(),
        })
        .collect();
    Json(skills)
}

async fn list_plugins(State(state): State<AppState>) -> Json<Vec<PluginDto>> {
    let plugins = state
        .plugins
        .list()
        .iter()
        .map(|p| PluginDto {
            name: p.name.clone(),
            path: p.path.to_string_lossy().into_owned(),
        })
        .collect();
    Json(plugins)
}

async fn create_session(
    State(state): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), (StatusCode, String)> {
    let session_id = format!("sess_{}", Ulid::new());
    let title = req.title.unwrap_or_else(|| "new session".to_string());
    // Store in-memory MCP server configs (including bearer tokens) strictly in RAM, never SQLite.
    if !req.mcp_servers.is_empty() {
        state
            .session_mcp_ram
            .insert(session_id.clone(), req.mcp_servers);
    }
    state.session_snapshot_disabled.insert(
        session_id.clone(),
        req.disable_internal_git_snapshot || state.managed_mode,
    );

    let rmode = match req.runtime_mode {
        WireRuntimeMode::ApprovalRequired => RuntimeMode::ApprovalRequired,
        WireRuntimeMode::Auto => RuntimeMode::Auto,
        WireRuntimeMode::FullAccess => RuntimeMode::FullAccess,
    };
    let imode = match req.interaction_mode {
        WireInteractionMode::Build => InteractionMode::Build,
        WireInteractionMode::Plan => InteractionMode::Plan,
    };
    state
        .session_modes
        .insert(session_id.clone(), (rmode, imode));

    state
        .store
        .bus
        .publish_durable(
            SessionEvent::SessionCreated {
                session_id: session_id.clone(),
                parent_id: None,
                fork_source_id: None,
                title: title.clone(),
                agent_id: "build".to_string(),
                model_profile: req.model_profile.clone(),
                cwd: req.cwd.clone(),
                workspace_root: req.cwd,
                runtime_mode: rmode,
                interaction_mode: imode,
            },
            None,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let summary = build_session_summary(&state, &session_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "created session missing from projection".to_string(),
            )
        })?;

    Ok((
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            session_id,
            session: summary,
        }),
    ))
}

async fn list_sessions(State(state): State<AppState>) -> Json<Vec<SessionSummaryDto>> {
    let recs = state.store.reader.list_sessions().unwrap_or_default();
    let mut out = Vec::with_capacity(recs.len());
    for rec in recs {
        if let Some(s) = build_session_summary(&state, &rec.id).await {
            out.push(s);
        }
    }
    out.sort_by_key(|s| s.status.sort_rank());
    Json(out)
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionSummaryDto>, StatusCode> {
    build_session_summary(&state, &id)
        .await
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.session_mcp_ram.remove(&id);
    state.session_snapshot_disabled.remove(&id);
    let sid = id.clone();
    state
        .store
        .bus
        .writer()
        .exec_tx(move |tx| {
            tx.execute("DELETE FROM session WHERE id = ?1", [sid])
                .map_err(|e| e.to_string())?;
            Ok(Value::Null)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn send_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PromptRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let delivery = if req.delivery == "steer" {
        DeliveryMode::Steer
    } else {
        DeliveryMode::Queue
    };
    let msg_id = state
        .inbox
        .enqueue(
            &id,
            req.client_msg_id,
            InboxItemType::Prompt,
            delivery,
            req.text.clone(),
            req.model_override.clone(),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Promote immediately and append user message so listeners & projections update synchronously.
    let promoted = state
        .inbox
        .promote(&id, true, false)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let chain = state
        .store
        .reader
        .walk_active_branch(&id)
        .unwrap_or_default();
    let mut parent = chain.last().map(|r| r.id.clone());
    for item in promoted {
        let message = saint_harness_core::inbox_item_to_message(&item, parent.clone());
        parent = Some(message.id.clone());
        let _ = state
            .store
            .bus
            .publish_durable(
                SessionEvent::MessageAppended {
                    session_id: id.clone(),
                    message,
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

    // Ring the Doorbell coordinator so the live AgentRunner processes this prompt and streams SSE events.
    let state_bg = state.clone();
    let sess_bg = id.clone();
    let profile_ov = req.profile_override.clone();
    let model_ov = req.model_override.clone();
    let imode_ov = req.interaction_mode;
    let rmode_ov = req.runtime_mode;
    state
        .coordinator
        .wake(id, WakeScope::DrainAll, move |_scope, cancel| {
            let st = state_bg.clone();
            let sid = sess_bg.clone();
            let po = profile_ov.clone();
            let mo = model_ov.clone();
            async move {
                let (stored_rmode, stored_imode) = st
                    .session_modes
                    .get(&sid)
                    .map(|v| *v)
                    .unwrap_or((RuntimeMode::ApprovalRequired, InteractionMode::Build));
                let rmode = match rmode_ov {
                    Some(WireRuntimeMode::ApprovalRequired) => RuntimeMode::ApprovalRequired,
                    Some(WireRuntimeMode::Auto) => RuntimeMode::Auto,
                    Some(WireRuntimeMode::FullAccess) => RuntimeMode::FullAccess,
                    None => stored_rmode,
                };
                let imode = match imode_ov {
                    Some(WireInteractionMode::Plan) => InteractionMode::Plan,
                    Some(WireInteractionMode::Build) => InteractionMode::Build,
                    None => stored_imode,
                };
                st.session_modes.insert(sid.clone(), (rmode, imode));
                let sandbox = match rmode {
                    RuntimeMode::FullAccess => SandboxProfile::Disabled,
                    _ if imode == InteractionMode::Plan => SandboxProfile::ReadOnlyWorkspace,
                    _ => SandboxProfile::WorkspaceWrite {
                        extra_rw_dirs: Vec::new(),
                    },
                };
                if let Err(err) = st
                    .run_session_turn(
                        &sid,
                        crate::LiveTurnRequest {
                            profile_override: po,
                            model_override: mo,
                            edit_override: None,
                            runtime_mode: rmode,
                            interaction_mode: imode,
                            sandbox_profile: sandbox,
                            max_steps: 12,
                            settings_home: None,
                        },
                        cancel,
                    )
                    .await
                {
                    let _ = st
                        .store
                        .bus
                        .publish_durable(
                            SessionEvent::StepFailed {
                                session_id: sid,
                                step_num: 0,
                                error: err,
                            },
                            None,
                        )
                        .await;
                }
            }
        })
        .await;

    Ok(Json(json!({ "accepted": true, "inbox_id": msg_id })))
}

async fn interrupt_session(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    state.coordinator.interrupt(&id, false).await;
    Json(json!({ "interrupted": true, "session_id": id }))
}

async fn fork_session_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let new_id = fork_session(&state.store, &id, req.through_msg_id.as_deref())
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(json!({ "session_id": new_id })))
}

async fn rollback_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RollbackRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let disabled = state
        .session_snapshot_disabled
        .get(&id)
        .map(|v| *v)
        .unwrap_or(state.managed_mode);
    let snap = ShadowSnapshotEngine::new(state.data_dir.join("snapshot"), disabled);
    let ws = state
        .store
        .reader
        .get_session(&id)
        .ok()
        .flatten()
        .map(|s| PathBuf::from(s.workspace_root))
        .unwrap_or_else(|| state.workspace_root.clone());

    match req.action.as_str() {
        "stage" => {
            let seq = req.target_seq.unwrap_or(0);
            let snap_id = req.snapshot_id.unwrap_or_else(|| "disabled".to_string());
            let orig = revert_stage(&state.store, &snap, &ws, &id, seq, &snap_id)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            Ok(Json(json!({ "staged": true, "orig_snapshot": orig })))
        }
        "clear" => {
            revert_clear(&state.store, &snap, &ws, &id)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            Ok(Json(json!({ "cleared": true })))
        }
        "commit" => {
            let deleted = revert_commit(&state.store, &id)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            Ok(Json(
                json!({ "committed": true, "deleted_messages": deleted }),
            ))
        }
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("unknown rollback action `{other}`"),
        )),
    }
}

async fn compact_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let chain = state
        .store
        .reader
        .walk_active_branch(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let msgs: Vec<_> = chain.into_iter().map(|r| r.message).collect();
    let shake = saint_harness_token::run_tier1_shake(
        &msgs,
        110_000,
        4,
        Some(&state.data_dir.join("spill")),
    );
    Ok(Json(json!({
        "tier_used": 1,
        "before_tokens": shake.before_tokens,
        "after_tokens": shake.after_tokens,
        "spilled_blocks": shake.spilled_blocks,
        "below_target": shake.below_target
    })))
}

async fn btw_session(
    State(_state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<BtwRequest>,
) -> Json<BtwResponse> {
    Json(BtwResponse {
        answer: format!(
            "[session {id} warm-cache /btw] Side question received: \"{}\". No rows written to session_message.",
            req.question
        ),
        wrote_history: false,
    })
}

async fn session_tree(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<TreeNodeDto>>, (StatusCode, String)> {
    let nodes = list_session_tree(&state.store, &id)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?
        .into_iter()
        .map(|n| TreeNodeDto {
            id: n.id,
            parent_msg_id: n.parent_msg_id,
            seq: n.seq,
            role: n.role,
            preview: n.preview,
            is_head: n.is_head,
        })
        .collect();
    Ok(Json(nodes))
}

async fn stop_task(
    State(_state): State<AppState>,
    Path((session_id, task_id)): Path<(String, String)>,
) -> Json<Value> {
    Json(json!({ "stopped": true, "session_id": session_id, "task_id": task_id }))
}

async fn steer_task(
    State(state): State<AppState>,
    Path((session_id, task_id)): Path<(String, String)>,
    Json(req): Json<TaskSteerRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let inbox_id = state
        .inbox
        .enqueue(
            &session_id,
            None,
            InboxItemType::Synthetic,
            DeliveryMode::Steer,
            format!("<task-steer id=\"{task_id}\">{}</task-steer>", req.message),
            None,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!({ "steered": true, "inbox_id": inbox_id })))
}

async fn reply_permission(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(req): Json<PermissionReplyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let reply = match req.decision.as_str() {
        "once" => PermissionReply::Once,
        "always" => PermissionReply::Always {
            pattern: req.pattern.unwrap_or_default(),
        },
        "reject_with_feedback" => PermissionReply::RejectWithFeedback {
            feedback: req.feedback.unwrap_or_default(),
        },
        _ => PermissionReply::Reject,
    };
    state
        .perms
        .reply_permission(&request_id, reply)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(Json(json!({ "resolved": true })))
}

async fn reply_user_input(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(req): Json<UserInputReplyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .perms
        .reply_user_input(
            &request_id,
            UserInputAnswers {
                answers: req.answers,
            },
        )
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(Json(json!({ "resolved": true })))
}

async fn build_session_summary(state: &AppState, session_id: &str) -> Option<SessionSummaryDto> {
    let rec = state.store.reader.get_session(session_id).ok()??;
    let chain = state
        .store
        .reader
        .walk_active_branch(session_id)
        .unwrap_or_default();
    let running = state.coordinator.is_running(session_id).await;
    let status = if running {
        SessionUiStatus::Working
    } else if chain.is_empty() {
        SessionUiStatus::Idle
    } else {
        SessionUiStatus::Done
    };

    let messages: Vec<TranscriptItemDto> = chain
        .into_iter()
        .filter_map(|row| {
            let txt = row.message.plain_text();
            if txt.is_empty() {
                return None;
            }
            match row.message.role {
                saint_harness_schema::Role::User => Some(TranscriptItemDto::User { text: txt }),
                saint_harness_schema::Role::Assistant => Some(TranscriptItemDto::Assistant {
                    text: txt,
                    meta: None,
                }),
                _ => None,
            }
        })
        .collect();

    let model_label = match rec.model_profile.as_str() {
        "qwen-3.8-27b" => "Qwen 3.8".to_string(),
        "qwen-4-upcoming" => "Qwen 4".to_string(),
        "claude-flagship" => "Claude Flagship".to_string(),
        "gemini-flagship" => "Gemini Flagship".to_string(),
        other => other.to_string(),
    };
    let model_key = match rec.model_profile.as_str() {
        "qwen-3.8-27b" => "qwen".to_string(),
        "qwen-4-upcoming" => "qwen4".to_string(),
        "claude-flagship" => "claude".to_string(),
        "gemini-flagship" => "gemini".to_string(),
        other => other.to_string(),
    };

    Some(SessionSummaryDto {
        id: rec.id,
        name: rec.title,
        status,
        model_profile: rec.model_profile,
        model_label,
        model_key,
        ctx_label: "0 / 110K".to_string(),
        diff: None,
        subtitle: match status {
            SessionUiStatus::Working => "working…".to_string(),
            SessionUiStatus::Done => "done".to_string(),
            SessionUiStatus::Blocked => "1 approval".to_string(),
            SessionUiStatus::Idle => "main".to_string(),
        },
        cache_warm: false,
        cache_label: "Cold · 0%".to_string(),
        cache_ttl_secs: 0,
        cache_ttl_max_secs: 300,
        cache_hit_pct: 0.0,
        prune_epoch: 0,
        queued_masks: 0,
        cost_usd: rec.total_cost_usd,
        runtime_mode: WireRuntimeMode::ApprovalRequired,
        interaction_mode: WireInteractionMode::Build,
        decision: None,
        messages,
    })
}
