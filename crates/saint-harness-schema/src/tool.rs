use crate::{InteractionMode, RuntimeMode, SandboxProfile, ToolOutcome, ToolResultBlock};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default)]
    pub defer_loading: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: Vec<ToolResultBlock>,
    #[serde(default)]
    pub raw_bytes: usize,
    #[serde(default)]
    pub condensed_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl ToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        let s = text.into();
        let len = s.len();
        Self {
            content: vec![ToolResultBlock::Text { text: s }],
            raw_bytes: len,
            condensed_bytes: len,
            metadata: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub call_id: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub additional_directories: Vec<PathBuf>,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: InteractionMode,
    pub sandbox_profile: SandboxProfile,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDef;
    fn permission_action(&self) -> &str {
        "tool"
    }
    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome;
}
