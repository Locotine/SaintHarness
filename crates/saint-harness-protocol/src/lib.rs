use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Pure wire DTOs shared between `saint-harness-server`, `saint-harness-tui`, and `saint-harness-cli`.
/// Law #1: Zero dependency on `saint-harness-core` or any leaf crate.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionUiStatus {
    Blocked,
    Working,
    Done,
    Idle,
}

impl SessionUiStatus {
    pub fn group_label(self) -> &'static str {
        match self {
            SessionUiStatus::Blocked => "Needs you",
            SessionUiStatus::Working => "Working",
            SessionUiStatus::Done => "Ready",
            SessionUiStatus::Idle => "Idle",
        }
    }

    pub fn sort_rank(self) -> u8 {
        match self {
            SessionUiStatus::Blocked => 0,
            SessionUiStatus::Working => 1,
            SessionUiStatus::Done => 2,
            SessionUiStatus::Idle => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WireRuntimeMode {
    #[default]
    ApprovalRequired,
    Auto,
    FullAccess,
}

impl WireRuntimeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WireRuntimeMode::ApprovalRequired => "approval-required",
            WireRuntimeMode::Auto => "auto",
            WireRuntimeMode::FullAccess => "full-access",
        }
    }

    pub fn display_pill(self) -> &'static str {
        match self {
            WireRuntimeMode::ApprovalRequired => "○ Ask for approval ▾",
            WireRuntimeMode::Auto => "● Auto mode ▾",
            WireRuntimeMode::FullAccess => "▲ Bypass permissions ▾",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireInteractionMode {
    #[default]
    Build,
    Plan,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStatusCounts {
    pub blocked: usize,
    pub working: usize,
    pub done: usize,
    pub idle: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub reachable: bool,
    pub ready: bool,
    pub version: String,
    pub cold_start_ms: u64,
    pub url: String,
    pub socket_path: Option<String>,
    pub projection_state: String,
    pub high_water_seq: i64,
    pub writer_queue: usize,
    pub sessions: SessionStatusCounts,
    pub catalog_summary: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChoiceDto {
    pub key: String,
    pub name: String,
    pub label: String,
    pub selector: String,
    pub resolved_id: Option<String>,
    pub context_window: Option<u64>,
    pub edit_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderGroupDto {
    pub key: String,
    pub label: String,
    pub catalog: String,
    pub tag: String,
    pub handoff_note: String,
    pub footnote: Option<String>,
    pub children: Vec<ModelChoiceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsCatalogResponse {
    pub hardcoded_provider_versions: usize,
    pub leaves: Vec<ModelChoiceDto>,
    pub providers: Vec<ModelProviderGroupDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStatDto {
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionCardDto {
    pub request_id: String,
    pub kind: String, // "shell" | "ask"
    pub title: String,
    pub command: Option<String>,
    pub sandbox: Option<String>,
    pub sandbox_info: Option<String>,
    pub pattern: Option<String>,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTraceDto {
    pub verb: String,
    pub arg: String,
    pub note: String,
    pub opens_diff: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum TranscriptItemDto {
    User {
        text: String,
    },
    Handoff {
        text: String,
    },
    Activity {
        summary: String,
        working: bool,
        expanded: bool,
        tools: Vec<ToolTraceDto>,
    },
    Assistant {
        text: String,
        meta: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryDto {
    pub id: String,
    pub name: String,
    pub status: SessionUiStatus,
    pub model_profile: String,
    pub model_label: String,
    pub model_key: String,
    pub ctx_label: String,
    pub diff: Option<DiffStatDto>,
    pub subtitle: String,
    pub cache_warm: bool,
    pub cache_label: String,
    pub cache_ttl_secs: u64,
    pub cache_ttl_max_secs: u64,
    pub cache_hit_pct: f64,
    pub prune_epoch: u64,
    pub queued_masks: usize,
    pub cost_usd: f64,
    pub runtime_mode: WireRuntimeMode,
    pub interaction_mode: WireInteractionMode,
    pub decision: Option<DecisionCardDto>,
    pub messages: Vec<TranscriptItemDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub cwd: String,
    #[serde(default)]
    pub additional_directories: Vec<String>,
    #[serde(default = "default_profile")]
    pub model_profile: String,
    #[serde(default)]
    pub interaction_mode: WireInteractionMode,
    #[serde(default)]
    pub runtime_mode: WireRuntimeMode,
    #[serde(default)]
    pub sandbox_mode: Option<String>,
    #[serde(default)]
    pub disable_internal_git_snapshot: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

fn default_profile() -> String {
    "qwen-3.8-27b".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub session: SessionSummaryDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRequest {
    pub text: String,
    #[serde(default = "default_delivery")]
    pub delivery: String,
    #[serde(default)]
    pub client_msg_id: Option<String>,
    #[serde(default)]
    pub model_override: Option<String>,
    #[serde(default)]
    pub interaction_mode: Option<WireInteractionMode>,
}

fn default_delivery() -> String {
    "queue".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkRequest {
    pub through_msg_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackRequest {
    pub action: String, // "stage" | "clear" | "commit"
    pub target_seq: Option<i64>,
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtwRequest {
    pub question: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtwResponse {
    pub answer: String,
    pub wrote_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionReplyRequest {
    pub decision: String, // "once" | "always" | "reject" | "reject_with_feedback"
    pub pattern: Option<String>,
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputReplyRequest {
    pub answers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSteerRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDto {
    pub id: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDto {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNodeDto {
    pub id: String,
    pub parent_msg_id: Option<String>,
    pub seq: i64,
    pub role: String,
    pub preview: String,
    pub is_head: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseWireEvent {
    pub stream_id: i64,
    pub event_type: String,
    pub session_id: String,
    pub payload: Value,
}
