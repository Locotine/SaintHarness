use crate::{
    InteractionMode, Message, PermissionRequest, RuntimeMode, ToolResultBlock, Usage,
    UserInputRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInputErrorKind {
    MalformedJson,
    TruncatedByLength,
    IncompleteStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LLMEvent {
    StepStart,
    TextStart,
    TextDelta {
        delta: String,
    },
    TextEnd,
    ReasoningStart,
    ReasoningDelta {
        delta: String,
    },
    ReasoningEnd {
        signature: Option<String>,
        redacted: Option<String>,
    },
    ToolInputStart {
        id: String,
        name: String,
    },
    ToolInputDelta {
        id: String,
        delta: String,
    },
    ToolInputError {
        id: String,
        name: String,
        raw_excerpt: String,
        reason: ToolInputErrorKind,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        provider_executed: bool,
        signature: Option<String>,
    },
    ToolResultHosted {
        call_id: String,
        content: Vec<ToolResultBlock>,
        is_error: bool,
    },
    Finish {
        reason: FinishReason,
        usage: Usage,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    SessionCreated {
        session_id: String,
        parent_id: Option<String>,
        fork_source_id: Option<String>,
        title: String,
        agent_id: String,
        model_profile: String,
        cwd: String,
        workspace_root: String,
        runtime_mode: RuntimeMode,
        interaction_mode: InteractionMode,
    },
    SessionTitleUpdated {
        session_id: String,
        title: String,
        is_default: bool,
    },
    SessionProfileSwitched {
        session_id: String,
        model_profile: String,
        model_id: Option<String>,
    },
    SessionActiveBranchChanged {
        session_id: String,
        active_branch: Option<String>,
    },
    MessageAppended {
        session_id: String,
        message: Message,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        error_json: Option<String>,
        snapshot_start: Option<String>,
        changed_files: Vec<String>,
    },
    StepStarted {
        session_id: String,
        step_num: u64,
    },
    StepEnded {
        session_id: String,
        step_num: u64,
        finish_reason: FinishReason,
        usage: Usage,
        cost_usd: f64,
    },
    StepFailed {
        session_id: String,
        step_num: u64,
        error: String,
    },
    PermissionRequested {
        session_id: String,
        request: PermissionRequest,
    },
    PermissionResolved {
        session_id: String,
        request_id: String,
        outcome: String,
    },
    UserInputRequested {
        session_id: String,
        request: UserInputRequest,
    },
    UserInputResolved {
        session_id: String,
        request_id: String,
    },
    TurnDiffUpdated {
        session_id: String,
        files_changed: usize,
        lines_added: usize,
        lines_removed: usize,
        unified_diff: String,
    },
    CacheMeterUpdated {
        session_id: String,
        warm: bool,
        ttl_remaining_secs: u64,
        hit_ratio: f64,
        prune_epoch: u64,
        queued_masks: usize,
    },
    FileReadRecorded {
        session_id: String,
        canonical_path: String,
        blake3_hex: String,
        line_count: usize,
        snapshot_zst: Option<Vec<u8>>,
        read_epoch: u64,
    },
    BackgroundJobUpdated {
        session_id: String,
        job_id: String,
        kind: String,
        command_preview: String,
        status: String,
        exit_code: Option<i32>,
        spill_path: Option<String>,
    },
    // Ephemeral events bypass SQLite and fan out directly over broadcast SSE.
    TextDelta {
        session_id: String,
        delta: String,
    },
    ReasoningDelta {
        session_id: String,
        delta: String,
    },
    ToolProgress {
        session_id: String,
        call_id: String,
        tool_name: String,
        status: String,
    },
}

impl SessionEvent {
    pub fn is_durable(&self) -> bool {
        !self.is_ephemeral()
    }

    pub fn is_ephemeral(&self) -> bool {
        matches!(
            self,
            SessionEvent::TextDelta { .. }
                | SessionEvent::ReasoningDelta { .. }
                | SessionEvent::ToolProgress { .. }
        )
    }

    pub fn session_id(&self) -> &str {
        match self {
            SessionEvent::SessionCreated { session_id, .. }
            | SessionEvent::SessionTitleUpdated { session_id, .. }
            | SessionEvent::SessionProfileSwitched { session_id, .. }
            | SessionEvent::SessionActiveBranchChanged { session_id, .. }
            | SessionEvent::MessageAppended { session_id, .. }
            | SessionEvent::StepStarted { session_id, .. }
            | SessionEvent::StepEnded { session_id, .. }
            | SessionEvent::StepFailed { session_id, .. }
            | SessionEvent::PermissionRequested { session_id, .. }
            | SessionEvent::PermissionResolved { session_id, .. }
            | SessionEvent::UserInputRequested { session_id, .. }
            | SessionEvent::UserInputResolved { session_id, .. }
            | SessionEvent::TurnDiffUpdated { session_id, .. }
            | SessionEvent::CacheMeterUpdated { session_id, .. }
            | SessionEvent::FileReadRecorded { session_id, .. }
            | SessionEvent::BackgroundJobUpdated { session_id, .. }
            | SessionEvent::TextDelta { session_id, .. }
            | SessionEvent::ReasoningDelta { session_id, .. }
            | SessionEvent::ToolProgress { session_id, .. } => session_id,
        }
    }

    pub fn event_type_name(&self) -> &'static str {
        match self {
            SessionEvent::SessionCreated { .. } => "session.created",
            SessionEvent::SessionTitleUpdated { .. } => "session.title_updated",
            SessionEvent::SessionProfileSwitched { .. } => "session.profile_switched",
            SessionEvent::SessionActiveBranchChanged { .. } => "session.active_branch_changed",
            SessionEvent::MessageAppended { .. } => "message.appended",
            SessionEvent::StepStarted { .. } => "step.started",
            SessionEvent::StepEnded { .. } => "step.ended",
            SessionEvent::StepFailed { .. } => "step.failed",
            SessionEvent::PermissionRequested { .. } => "permission.requested",
            SessionEvent::PermissionResolved { .. } => "permission.resolved",
            SessionEvent::UserInputRequested { .. } => "user_input.requested",
            SessionEvent::UserInputResolved { .. } => "user_input.resolved",
            SessionEvent::TurnDiffUpdated { .. } => "turn.diff.updated",
            SessionEvent::CacheMeterUpdated { .. } => "cache.meter_updated",
            SessionEvent::FileReadRecorded { .. } => "file.read_recorded",
            SessionEvent::BackgroundJobUpdated { .. } => "background_job.updated",
            SessionEvent::TextDelta { .. } => "text.delta",
            SessionEvent::ReasoningDelta { .. } => "reasoning.delta",
            SessionEvent::ToolProgress { .. } => "tool.progress",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitReceipt {
    pub event_id: String,
    pub aggregate_id: String,
    pub seq: i64,
    pub created_at: i64,
}
