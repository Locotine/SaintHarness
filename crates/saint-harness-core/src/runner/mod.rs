pub mod attempt;
pub mod doom_loop;
pub mod retry;
pub mod step;

pub use attempt::*;
pub use doom_loop::*;
pub use retry::*;
pub use step::*;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::store::EventStore;
    use async_trait::async_trait;
    use futures::stream;
    use saint_harness_schema::{
        AiError, FinishReason, InteractionMode, LLMEvent, LlmRequest, LlmStream, NoopPolicyGuard,
        ResolvedModel, RuntimeMode, SandboxProfile, SessionEvent, Tool, ToolContext, ToolDef,
        ToolOutcome, ToolResult, TransportOp, Usage, WireProtocol, embedded_profile,
        embedded_routes,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    struct ScriptedProto {
        turns: Vec<Vec<Result<LLMEvent, AiError>>>,
        idx: AtomicUsize,
    }
    #[async_trait]
    impl WireProtocol for ScriptedProto {
        fn protocol_id(&self) -> &str {
            "mock"
        }
        async fn stream(&self, _req: LlmRequest) -> Result<LlmStream, AiError> {
            let i = self.idx.fetch_add(1, Ordering::SeqCst);
            let evs = self.turns.get(i).cloned().unwrap_or_default();
            Ok(Box::pin(stream::iter(evs)))
        }
    }

    struct DecliningTool;
    #[async_trait]
    impl Tool for DecliningTool {
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "danger".into(),
                description: "declines".into(),
                input_schema: json!({"type":"object"}),
                defer_loading: false,
                server_name: None,
            }
        }
        async fn execute(&self, _ctx: &ToolContext, _args: Value) -> ToolOutcome {
            ToolOutcome::UserDeclined
        }
    }

    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "echo".into(),
                description: "echoes".into(),
                input_schema: json!({"type":"object"}),
                defer_loading: false,
                server_name: None,
            }
        }
        async fn execute(&self, _ctx: &ToolContext, _args: Value) -> ToolOutcome {
            ToolOutcome::Completed(ToolResult::text("ok"))
        }
    }

    #[test]
    fn retry_budgets_are_independent_and_doom_loop_warns_at_3_halts_at_5() {
        let mut budgets = RetryBudgets::default();
        // Network retries do not consume general budget
        let net_err = AiError::Transport {
            operation: TransportOp::Dns,
            accepted: false,
            network_down: true,
            message: "dns".into(),
        };
        assert!(budgets.next_delay(&net_err, 1.0).is_some());
        assert_eq!(budgets.general_attempts, 0);
        // Timeout caps at 3
        let to_err = AiError::Timeout {
            message: "t".into(),
        };
        assert!(budgets.next_delay(&to_err, 1.0).is_some());
        assert!(budgets.next_delay(&to_err, 1.0).is_some());
        assert!(budgets.next_delay(&to_err, 1.0).is_some());
        assert!(budgets.next_delay(&to_err, 1.0).is_none());
        // x-should-retry=false disables retry immediately
        let no_retry = AiError::RateLimit {
            message: "rl".into(),
            retry_after_ms: Some(1000),
            should_retry_header: Some(false),
        };
        assert!(budgets.next_delay(&no_retry, 1.0).is_none());

        let mut doom = DoomLoopDetector::default();
        let args = json!({"b": 2, "a": 1});
        assert_eq!(doom.record("read", &args), DoomLoopStatus::Ok);
        assert_eq!(doom.record("read", &args), DoomLoopStatus::Ok);
        assert_eq!(
            doom.record("read", &args),
            DoomLoopStatus::Warn { repeats: 3 }
        );
        assert_eq!(
            doom.record("read", &args),
            DoomLoopStatus::Warn { repeats: 4 }
        );
        assert_eq!(
            doom.record("read", &args),
            DoomLoopStatus::Halt { repeats: 5 }
        );
    }

    #[tokio::test]
    async fn runner_handles_user_declined_and_3_strike_output_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("run.sqlite")).unwrap();
        let profile = embedded_profile("qwen-3.8-27b").unwrap();
        let routes = embedded_routes().unwrap();
        let route = routes.routes.get("qwen_local_sglang").unwrap().clone();
        let model = ResolvedModel {
            provider: "qwen".into(),
            model_id: "qwen3.8-27b".into(),
            display_name: "Qwen".into(),
            native_window: 262_144,
            effective_window: 110_000,
            max_output_tokens: 16_384,
            supports_adaptive_thinking: false,
            supports_vision: false,
            resolution_source: "test".into(),
        };

        // Case 1: ToolOutcome::UserDeclined halts turn immediately
        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: "s_dec".into(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "dec".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: "/tmp".into(),
                    workspace_root: "/tmp".into(),
                    runtime_mode: RuntimeMode::ApprovalRequired,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();

        let mut tools: BTreeMap<String, Arc<dyn Tool>> = BTreeMap::new();
        tools.insert("danger".into(), Arc::new(DecliningTool));
        tools.insert("echo".into(), Arc::new(EchoTool));

        let deps = AttemptDeps {
            store: store.clone(),
            protocol: Arc::new(ScriptedProto {
                turns: vec![vec![
                    Ok(LLMEvent::ToolCall {
                        id: "c1".into(),
                        name: "danger".into(),
                        arguments: json!({}),
                        provider_executed: false,
                        signature: None,
                    }),
                    Ok(LLMEvent::Finish {
                        reason: FinishReason::ToolCalls,
                        usage: Usage::default(),
                    }),
                ]],
                idx: AtomicUsize::new(0),
            }),
            tools: tools.clone(),
            policy_guard: Arc::new(NoopPolicyGuard),
        };
        let cfg = TurnConfig {
            session_id: "s_dec".into(),
            system_blocks: vec!["sys".into()],
            model: model.clone(),
            profile: profile.clone(),
            route: route.clone(),
            tool_ctx: ToolContext {
                session_id: "s_dec".into(),
                call_id: String::new(),
                cwd: PathBuf::from("/tmp"),
                workspace_root: PathBuf::from("/tmp"),
                additional_directories: vec![],
                runtime_mode: RuntimeMode::ApprovalRequired,
                interaction_mode: InteractionMode::Build,
                sandbox_profile: SandboxProfile::Disabled,
            },
            max_steps: 5,
            smol_mode: false,
        };
        let res = run_turn(&deps, &cfg, CancellationToken::new())
            .await
            .unwrap();
        assert!(res.stopped_by_user_declined);
        assert_eq!(res.steps_executed, 1);

        // Case 2: 3 consecutive FinishReason::Length triggers OutputLimit stop
        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: "s_len".into(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "len".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: "/tmp".into(),
                    workspace_root: "/tmp".into(),
                    runtime_mode: RuntimeMode::Auto,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();
        let len_turn = vec![
            Ok(LLMEvent::ToolCall {
                id: "e1".into(),
                name: "echo".into(),
                arguments: json!({}),
                provider_executed: false,
                signature: None,
            }),
            Ok(LLMEvent::Finish {
                reason: FinishReason::Length,
                usage: Usage::default(),
            }),
        ];
        let deps2 = AttemptDeps {
            store: store.clone(),
            protocol: Arc::new(ScriptedProto {
                turns: vec![len_turn.clone(), len_turn.clone(), len_turn],
                idx: AtomicUsize::new(0),
            }),
            tools,
            policy_guard: Arc::new(NoopPolicyGuard),
        };
        let mut cfg2 = cfg;
        cfg2.session_id = "s_len".into();
        let res2 = run_turn(&deps2, &cfg2, CancellationToken::new())
            .await
            .unwrap();
        assert!(res2.stopped_by_output_limit);
        assert_eq!(res2.steps_executed, 3);
    }
}
