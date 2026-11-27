use crate::AppState;
use saint_harness_core::{
    AskUserTool, AttemptDeps, EditTool, GlobTool, GrepTool, InstructionState, ReadTool,
    RepoMapTool, ShellTool, TurnConfig, TurnResult, WriteTool, run_turn,
};
use saint_harness_schema::{
    InteractionMode, PermissionPolicy, RuntimeMode, SandboxProfile, Tool, ToolContext,
    embedded_profile, load_routes_with_overrides, resolve_env_value,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct LiveTurnRequest {
    pub profile_override: Option<String>,
    pub model_override: Option<String>,
    pub edit_override: Option<String>,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: InteractionMode,
    pub sandbox_profile: SandboxProfile,
    pub max_steps: usize,
    pub settings_home: Option<PathBuf>,
}

impl Default for LiveTurnRequest {
    fn default() -> Self {
        Self {
            profile_override: None,
            model_override: None,
            edit_override: None,
            runtime_mode: RuntimeMode::ApprovalRequired,
            interaction_mode: InteractionMode::Build,
            sandbox_profile: SandboxProfile::WorkspaceWrite {
                extra_rw_dirs: Vec::new(),
            },
            max_steps: 12,
            settings_home: None,
        }
    }
}

impl AppState {
    /// Builds the full local coding-agent toolset wired to the server's leaf ports.
    pub fn build_tools(
        &self,
        edit_strategy: &str,
        policy: PermissionPolicy,
    ) -> BTreeMap<String, Arc<dyn Tool>> {
        let mut tools: BTreeMap<String, Arc<dyn Tool>> = BTreeMap::new();
        tools.insert(
            "read".to_string(),
            Arc::new(ReadTool::new(
                self.store.clone(),
                Some(self.outliner.clone()),
                200,
                true,
                Arc::new(AtomicU64::new(0)),
            )),
        );
        tools.insert("grep".to_string(), Arc::new(GrepTool));
        tools.insert("glob".to_string(), Arc::new(GlobTool));
        tools.insert(
            "edit".to_string(),
            Arc::new(EditTool {
                writer: self.file_writer.clone(),
                default_strategy: edit_strategy.to_string(),
                store: self.store.clone(),
                perms: self.perms.clone(),
                policy: policy.clone(),
            }),
        );
        tools.insert(
            "write".to_string(),
            Arc::new(WriteTool {
                writer: self.file_writer.clone(),
                store: self.store.clone(),
                perms: self.perms.clone(),
                policy: policy.clone(),
            }),
        );
        tools.insert(
            "shell".to_string(),
            Arc::new(ShellTool {
                runner: self.shell_runner.clone(),
                perms: self.perms.clone(),
                policy,
                spill_dir: Some(self.data_dir.join("spill")),
            }),
        );
        tools.insert(
            "repo_map".to_string(),
            Arc::new(RepoMapTool {
                mapper: self.repo_mapper.clone(),
                default_budget_tokens: 1_500,
            }),
        );
        tools.insert(
            "ask_user".to_string(),
            Arc::new(AskUserTool {
                perms: self.perms.clone(),
            }),
        );
        tools
    }

    /// Executes a live multi-step agent turn against the configured provider route and local toolset.
    pub async fn run_session_turn(
        &self,
        session_id: &str,
        opts: LiveTurnRequest,
        cancel: CancellationToken,
    ) -> Result<TurnResult, String> {
        let rec = self
            .store
            .reader
            .get_session(session_id)?
            .ok_or_else(|| format!("session `{session_id}` not found"))?;

        let profile_name = opts
            .profile_override
            .as_deref()
            .unwrap_or(&rec.model_profile);
        let profile = embedded_profile(profile_name).map_err(|e| e.to_string())?;
        let ws = PathBuf::from(&rec.workspace_root);
        let cwd = PathBuf::from(&rec.cwd);

        let routes = load_routes_with_overrides(Some(&ws), opts.settings_home.as_deref())
            .map_err(|e| e.to_string())?;
        let route = routes
            .routes
            .get(&profile.model.route)
            .cloned()
            .ok_or_else(|| format!("unknown route `{}`", profile.model.route))?;
        let is_local_route =
            route.base_url.contains("127.0.0.1") || route.base_url.contains("localhost");
        if !is_local_route
            && resolve_env_value(&route.api_key_env, Some(&ws), opts.settings_home.as_deref())
                .is_none()
        {
            return Err(format!(
                "missing API key `{}` for route `{}` ({}). Run `saint-harness auth login` to save it in ~/.saint-harness/settings.json.",
                route.api_key_env, profile.model.route, route.base_url
            ));
        }
        let protocol = self
            .protocols
            .get(&route.protocol)
            .ok_or_else(|| format!("unsupported wire protocol `{}`", route.protocol))?;

        let resolved_model = self
            .model_resolver
            .resolve(&profile, &route, opts.model_override.as_deref(), false)
            .await;

        let edit_strategy = opts
            .edit_override
            .as_deref()
            .unwrap_or(&profile.tools.edit_mode);
        let policy = PermissionPolicy {
            runtime_mode: opts.runtime_mode,
            interaction_mode: opts.interaction_mode,
            ..PermissionPolicy::default()
        };
        let tools = self.build_tools(edit_strategy, policy);

        let tool_guidance = format!(
            "Preferred edit strategy: {edit_strategy}. Tools available: read, grep, glob, edit, write, shell, repo_map, ask_user."
        );
        let inst = InstructionState::initialize(
            &ws,
            &profile,
            &tool_guidance,
            opts.interaction_mode,
            None,
        );

        let deps = AttemptDeps {
            store: self.store.clone(),
            protocol,
            tools,
            policy_guard: self.policy_guard.clone(),
        };
        let cfg = TurnConfig {
            session_id: session_id.to_string(),
            system_blocks: vec![inst.system_block_0, inst.system_block_1_frozen],
            model: resolved_model,
            profile,
            route,
            tool_ctx: ToolContext {
                session_id: session_id.to_string(),
                call_id: String::new(),
                cwd,
                workspace_root: ws,
                additional_directories: Vec::new(),
                runtime_mode: opts.runtime_mode,
                interaction_mode: opts.interaction_mode,
                sandbox_profile: opts.sandbox_profile,
            },
            max_steps: opts.max_steps.max(1),
            smol_mode: false,
        };

        run_turn(&deps, &cfg, cancel).await
    }
}
