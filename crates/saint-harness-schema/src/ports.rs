use crate::{
    AiError, LLMEvent, Message, ModelProfile, ResolvedModel, RouteDescriptor, SandboxProfile,
    ToolDef,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub system: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub tool_choice: ToolChoice,
    pub max_output_tokens: u64,
    pub model: ResolvedModel,
    pub profile: ModelProfile,
    pub route: RouteDescriptor,
    pub session_affinity: String,
    pub smol_mode: bool,
    pub extra_headers: BTreeMap<String, String>,
}

pub type LlmStream = BoxStream<'static, Result<LLMEvent, AiError>>;

#[async_trait]
pub trait WireProtocol: Send + Sync {
    fn protocol_id(&self) -> &str;
    async fn stream(&self, request: LlmRequest) -> Result<LlmStream, AiError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditRequest {
    pub path: PathBuf,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditDraft {
    pub path: PathBuf,
    pub original_text: String,
    pub updated_text: String,
    pub strategy_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriteResult {
    pub path: PathBuf,
    pub unified_diff: String,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub blake3_hex: String,
    pub diagnostics: Vec<DiagnosticItem>,
    pub reformatted: bool,
}

pub trait EditStrategy: Send + Sync {
    fn name(&self) -> &str;
    fn compute_draft(&self, original: &str, request: &EditRequest) -> Result<EditDraft, String>;
}

#[async_trait]
pub trait FileWritePort: Send + Sync {
    async fn apply_edit(
        &self,
        strategy_name: &str,
        request: EditRequest,
    ) -> Result<FileWriteResult, String>;

    async fn write_new_file(
        &self,
        path: PathBuf,
        content: String,
    ) -> Result<FileWriteResult, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticItem {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: String,
}

pub trait CodeOutliner: Send + Sync {
    fn outline(&self, path: &Path, source: &str) -> Result<Option<String>, String>;
    fn check_syntax(&self, path: &Path, source: &str) -> Vec<DiagnosticItem>;
}

pub trait RepoMapper: Send + Sync {
    fn build_repo_map(
        &self,
        workspace_root: &Path,
        budget_tokens: usize,
        focus_files: &[PathBuf],
    ) -> Result<String, String>;
}

#[async_trait]
pub trait LspPort: Send + Sync {
    async fn diagnostics_after_write(&self, path: &Path) -> Vec<DiagnosticItem>;
    async fn symbol_read(&self, path: &Path, symbol_name: &str) -> Result<SymbolLocation, String>;
    async fn find_callers(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Vec<SymbolLocation>, String>;
}

#[async_trait]
pub trait FastLintPort: Send + Sync {
    async fn lint_file(&self, path: &Path, content: &str) -> Vec<DiagnosticItem>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellRequest {
    pub command: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub sandbox: SandboxProfile,
    pub timeout_secs: u64,
    pub spill_dir: Option<PathBuf>,
    pub env_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out_to_background: bool,
    pub background_job_id: Option<String>,
    pub spill_path: Option<PathBuf>,
}

#[async_trait]
pub trait SandboxRunner: Send + Sync {
    async fn run(&self, req: ShellRequest) -> Result<ShellOutput, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptResult {
    pub output: Value,
    pub tool_calls_made: usize,
    pub logs: Vec<String>,
}

#[async_trait]
pub trait ToolInvoker: Send + Sync {
    async fn invoke(&self, namespace: &str, name: &str, args: Value) -> Result<Value, String>;
}

#[async_trait]
pub trait ScriptHost: Send + Sync {
    async fn execute_script(
        &self,
        code: &str,
        timeout_ms: u64,
        max_tool_calls: usize,
        invoker: std::sync::Arc<dyn ToolInvoker>,
    ) -> Result<ScriptResult, String>;
}

#[async_trait]
pub trait SnapshotEngine: Send + Sync {
    async fn capture(&self, workspace_root: &Path, session_id: &str) -> Result<String, String>;
    async fn restore(
        &self,
        workspace_root: &Path,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<String>, String>;
    async fn diff_from(
        &self,
        workspace_root: &Path,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<String, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexHit {
    pub path: String,
    pub score: f32,
    pub snippet: String,
}

#[async_trait]
pub trait MonorepoIndex: Send + Sync {
    async fn index_workspace(&self, workspace_root: &Path) -> Result<usize, String>;
    async fn update_file(&self, workspace_root: &Path, rel_path: &str) -> Result<(), String>;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<IndexHit>, String>;
}
