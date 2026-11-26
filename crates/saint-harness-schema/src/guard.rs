use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    Pass,
    Transform(String),
    Block { reason: String },
}

#[async_trait]
pub trait PolicyGuard: Send + Sync {
    async fn inspect_input(&self, _session_id: &str, _text: &str) -> GuardDecision {
        GuardDecision::Pass
    }

    async fn inspect_tool_call(&self, _tool: &str, _args: &Value) -> GuardDecision {
        GuardDecision::Pass
    }

    async fn inspect_output(&self, _session_id: &str, _text: &str) -> GuardDecision {
        GuardDecision::Pass
    }
}

#[derive(Debug, Clone, Default)]
pub struct NoopPolicyGuard;

impl PolicyGuard for NoopPolicyGuard {}
