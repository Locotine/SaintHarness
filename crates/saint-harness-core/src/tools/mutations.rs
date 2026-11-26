use crate::permission::{ApprovalDecision, PermissionService};
use crate::store::EventStore;
use async_trait::async_trait;
use saint_harness_schema::{
    EditRequest, FileWritePort, PermissionPolicy, RepoMapper, SandboxRunner, SessionEvent,
    ShellRequest, Tool, ToolContext, ToolDef, ToolFailure, ToolOutcome, ToolResult,
    UserInputOption, UserInputQuestion, UserInputRequest,
};
use saint_harness_token::{
    DEFAULT_HEAD_BYTES, DEFAULT_TAIL_BYTES, condense_cli_output, truncate_bookend_with_spill,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ulid::Ulid;

pub struct EditTool {
    pub writer: Arc<dyn FileWritePort>,
    pub default_strategy: String,
    pub store: EventStore,
    pub perms: PermissionService,
    pub policy: PermissionPolicy,
}

#[async_trait]
impl Tool for EditTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "edit".to_string(),
            description: "Edit a file atomically via FileWriteCoordinator (hashline, str_replace, replace_symbol, or ast_edit).".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "strategy": { "type": "string", "enum": ["hashline", "str_replace", "replace_symbol", "ast_edit"] },
                    "payload": { "type": "object" }
                },
                "required": ["path", "payload"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "edit"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(raw_path) = arguments.get("path").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "edit requires `path`".to_string(),
            });
        };
        let strategy = arguments
            .get("strategy")
            .and_then(|v| v.as_str())
            .unwrap_or(&self.default_strategy);
        let payload = arguments
            .get("payload")
            .cloned()
            .unwrap_or_else(|| arguments.clone());
        let path = resolve_path(&ctx.cwd, raw_path);

        let mut eff_policy = self.policy.clone();
        eff_policy.runtime_mode = ctx.runtime_mode;
        eff_policy.interaction_mode = ctx.interaction_mode;
        eff_policy.additional_directories = ctx.additional_directories.clone();

        match self
            .perms
            .check_or_request(
                &eff_policy,
                &ctx.workspace_root.to_string_lossy(),
                &ctx.workspace_root,
                &ctx.session_id,
                &ctx.call_id,
                "edit",
                &path.to_string_lossy(),
                format!("Approve edit to {raw_path}?"),
                Some(raw_path.to_string()),
                None,
                None,
                None,
            )
            .await
        {
            Ok(ApprovalDecision::Allowed) => {}
            Ok(ApprovalDecision::UserDeclined) => return ToolOutcome::UserDeclined,
            Ok(ApprovalDecision::RejectedWithFeedback(f)) => {
                return ToolOutcome::RecoverableError(f);
            }
            Err(e) => {
                return ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e });
            }
        }

        match self
            .writer
            .apply_edit(strategy, EditRequest { path, payload })
            .await
        {
            Ok(res) => {
                let _ = self
                    .store
                    .bus
                    .publish_durable(
                        SessionEvent::TurnDiffUpdated {
                            session_id: ctx.session_id.clone(),
                            files_changed: 1,
                            lines_added: res.lines_added,
                            lines_removed: res.lines_removed,
                            unified_diff: res.unified_diff.clone(),
                        },
                        None,
                    )
                    .await;
                let diag_summary = if res.diagnostics.is_empty() {
                    "0 diagnostics".to_string()
                } else {
                    res.diagnostics
                        .iter()
                        .map(|d| format!("L{}:{}: {} ({})", d.line, d.column, d.message, d.source))
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                ToolOutcome::Completed(ToolResult::text(format!(
                    "Edited {} (+{} -{}, blake3={})\nDiagnostics: {}\n{}",
                    raw_path,
                    res.lines_added,
                    res.lines_removed,
                    &res.blake3_hex[..12],
                    diag_summary,
                    res.unified_diff
                )))
            }
            Err(e) => {
                if e.contains("StaleAnchorError") {
                    ToolOutcome::RecoverableError(ToolFailure::StaleAnchorError { message: e })
                } else {
                    ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e })
                }
            }
        }
    }
}

pub struct WriteTool {
    pub writer: Arc<dyn FileWritePort>,
    pub store: EventStore,
    pub perms: PermissionService,
    pub policy: PermissionPolicy,
}

#[async_trait]
impl Tool for WriteTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "write".to_string(),
            description: "Create or overwrite a file atomically via FileWriteCoordinator."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "write"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let (Some(raw_path), Some(content)) = (
            arguments.get("path").and_then(|v| v.as_str()),
            arguments.get("content").and_then(|v| v.as_str()),
        ) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "write requires `path` and `content`".to_string(),
            });
        };
        let path = resolve_path(&ctx.cwd, raw_path);
        let mut eff_policy = self.policy.clone();
        eff_policy.runtime_mode = ctx.runtime_mode;
        eff_policy.interaction_mode = ctx.interaction_mode;

        match self
            .perms
            .check_or_request(
                &eff_policy,
                &ctx.workspace_root.to_string_lossy(),
                &ctx.workspace_root,
                &ctx.session_id,
                &ctx.call_id,
                "write",
                &path.to_string_lossy(),
                format!("Approve writing {raw_path}?"),
                Some(raw_path.to_string()),
                None,
                None,
                None,
            )
            .await
        {
            Ok(ApprovalDecision::Allowed) => {}
            Ok(ApprovalDecision::UserDeclined) => return ToolOutcome::UserDeclined,
            Ok(ApprovalDecision::RejectedWithFeedback(f)) => {
                return ToolOutcome::RecoverableError(f);
            }
            Err(e) => {
                return ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e });
            }
        }

        match self.writer.write_new_file(path, content.to_string()).await {
            Ok(res) => {
                let _ = self
                    .store
                    .bus
                    .publish_durable(
                        SessionEvent::TurnDiffUpdated {
                            session_id: ctx.session_id.clone(),
                            files_changed: 1,
                            lines_added: res.lines_added,
                            lines_removed: res.lines_removed,
                            unified_diff: res.unified_diff,
                        },
                        None,
                    )
                    .await;
                ToolOutcome::Completed(ToolResult::text(format!(
                    "Wrote {} (+{} -{})",
                    raw_path, res.lines_added, res.lines_removed
                )))
            }
            Err(e) => ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e }),
        }
    }
}

pub struct RepoMapTool {
    pub mapper: Arc<dyn RepoMapper>,
    pub default_budget_tokens: usize,
}

#[async_trait]
impl Tool for RepoMapTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "repo_map".to_string(),
            description: "Generate a PageRank-ranked structural symbol map of the workspace within token budget.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "budget_tokens": { "type": "integer" },
                    "focus_files": { "type": "array", "items": { "type": "string" } }
                }
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "repo_map"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let budget = arguments
            .get("budget_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(self.default_budget_tokens);
        let focus: Vec<PathBuf> = arguments
            .get("focus_files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(PathBuf::from))
                    .collect()
            })
            .unwrap_or_default();

        match self
            .mapper
            .build_repo_map(&ctx.workspace_root, budget, &focus)
        {
            Ok(text) => ToolOutcome::Completed(ToolResult::text(text)),
            Err(e) => ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e }),
        }
    }
}

pub struct AskUserTool {
    pub perms: PermissionService,
}

#[async_trait]
impl Tool for AskUserTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "ask_user".to_string(),
            description:
                "Ask the user a structured clarifying question and wait for their selection."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "options": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string" },
                                "description": { "type": "string" }
                            },
                            "required": ["label"]
                        }
                    }
                },
                "required": ["prompt", "options"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "ask_user"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(prompt) = arguments.get("prompt").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "ask_user requires `prompt`".to_string(),
            });
        };
        let options: Vec<UserInputOption> = arguments
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| {
                        let label = o.get("label")?.as_str()?.to_string();
                        let description = o
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(UserInputOption { label, description })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let req = UserInputRequest {
            request_id: format!("ask_{}", Ulid::new()),
            session_id: ctx.session_id.clone(),
            questions: vec![UserInputQuestion {
                id: "q1".to_string(),
                prompt: prompt.to_string(),
                options,
                multi_select: false,
            }],
        };

        match self.perms.request_user_input(req).await {
            Ok(ans) => ToolOutcome::Completed(ToolResult::text(
                serde_json::to_string(&ans).unwrap_or_else(|_| "{}".to_string()),
            )),
            Err(e) => ToolOutcome::RecoverableError(ToolFailure::Interrupted { message: e }),
        }
    }
}

pub struct ShellTool {
    pub runner: Arc<dyn SandboxRunner>,
    pub perms: PermissionService,
    pub policy: PermissionPolicy,
    pub spill_dir: Option<PathBuf>,
}

#[async_trait]
impl Tool for ShellTool {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: "shell".to_string(),
            description: "Execute a shell command inside the OS sandbox (Landlock/Seatbelt/Windows) with write-time semantic condensation.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_secs": { "type": "integer" }
                },
                "required": ["command"]
            }),
            defer_loading: false,
            server_name: None,
        }
    }

    fn permission_action(&self) -> &str {
        "shell"
    }

    async fn execute(&self, ctx: &ToolContext, arguments: Value) -> ToolOutcome {
        let Some(command) = arguments.get("command").and_then(|v| v.as_str()) else {
            return ToolOutcome::RecoverableError(ToolFailure::ValidationError {
                message: "shell requires string parameter `command`".to_string(),
            });
        };
        let timeout_secs = arguments
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let mut eff_policy = self.policy.clone();
        eff_policy.runtime_mode = ctx.runtime_mode;
        eff_policy.interaction_mode = ctx.interaction_mode;

        match self
            .perms
            .check_or_request(
                &eff_policy,
                &ctx.workspace_root.to_string_lossy(),
                &ctx.workspace_root,
                &ctx.session_id,
                &ctx.call_id,
                "shell",
                command,
                "Approve this command?".to_string(),
                None,
                Some(format!("{:?}", ctx.sandbox_profile)),
                None,
                None,
            )
            .await
        {
            Ok(ApprovalDecision::Allowed) => {}
            Ok(ApprovalDecision::UserDeclined) => return ToolOutcome::UserDeclined,
            Ok(ApprovalDecision::RejectedWithFeedback(f)) => {
                return ToolOutcome::RecoverableError(f);
            }
            Err(e) => {
                return ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e });
            }
        }

        let res = self
            .runner
            .run(ShellRequest {
                command: command.to_string(),
                cwd: ctx.cwd.clone(),
                workspace_root: ctx.workspace_root.clone(),
                sandbox: ctx.sandbox_profile.clone(),
                timeout_secs,
                spill_dir: self.spill_dir.clone(),
                env_allowlist: vec![],
            })
            .await;

        match res {
            Ok(out) => {
                let combined = if out.stderr.is_empty() {
                    out.stdout
                } else {
                    format!("{}\n{}", out.stdout, out.stderr)
                };
                let condensed = condense_cli_output(command, &combined);
                let spill_id = Ulid::new().to_string();
                let truncated = truncate_bookend_with_spill(
                    &condensed.text,
                    DEFAULT_HEAD_BYTES,
                    DEFAULT_TAIL_BYTES,
                    self.spill_dir.as_deref(),
                    Some(&spill_id),
                );
                let header = format!(
                    "exit={:?} raw={}B condensed={}B\n",
                    out.exit_code, condensed.raw_bytes, condensed.condensed_bytes
                );
                let full = format!("{header}{}", truncated.text);
                ToolOutcome::Completed(ToolResult {
                    content: vec![saint_harness_schema::ToolResultBlock::Text { text: full }],
                    raw_bytes: condensed.raw_bytes,
                    condensed_bytes: condensed.condensed_bytes,
                    metadata: None,
                })
            }
            Err(e) => ToolOutcome::RecoverableError(ToolFailure::RuntimeError { message: e }),
        }
    }
}

fn resolve_path(cwd: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}
