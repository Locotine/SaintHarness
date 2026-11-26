use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionEffect {
    Allow = 0,
    Ask = 1,
    Deny = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeMode {
    #[default]
    ApprovalRequired,
    Auto,
    FullAccess,
}

impl RuntimeMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RuntimeMode::ApprovalRequired => "approval-required",
            RuntimeMode::Auto => "auto",
            RuntimeMode::FullAccess => "full-access",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    #[default]
    Build,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "profile", rename_all = "snake_case")]
pub enum SandboxProfile {
    ReadOnlyWorkspace,
    WorkspaceWrite {
        #[serde(default)]
        extra_rw_dirs: Vec<PathBuf>,
    },
    Disabled,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self::WorkspaceWrite {
            extra_rw_dirs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRule {
    pub action: String,
    pub resource_pattern: String,
    pub effect: PermissionEffect,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPolicy {
    #[serde(default)]
    pub rules: Vec<PermissionRule>,
    #[serde(default)]
    pub override_rules: Vec<PermissionRule>,
    #[serde(default)]
    pub restrict_rules: Vec<PermissionRule>,
    pub fallback: Option<PermissionEffect>,
    #[serde(default)]
    pub additional_directories: Vec<PathBuf>,
    #[serde(default)]
    pub auto_approve_servers: Vec<String>,
    #[serde(default)]
    pub runtime_mode: RuntimeMode,
    #[serde(default)]
    pub interaction_mode: InteractionMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub request_id: String,
    pub session_id: String,
    pub message_id: String,
    pub action: String,
    pub resource: String,
    pub title: String,
    pub suggested_pattern: Option<String>,
    pub sandbox_summary: Option<String>,
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionReply {
    Once,
    Always { pattern: String },
    Reject,
    RejectWithFeedback { feedback: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInputQuestion {
    pub id: String,
    pub prompt: String,
    pub options: Vec<UserInputOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInputRequest {
    pub request_id: String,
    pub session_id: String,
    pub questions: Vec<UserInputQuestion>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserInputAnswers {
    pub answers: BTreeMap<String, Vec<String>>,
}
