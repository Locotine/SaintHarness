pub mod auth;
pub mod routes;
pub mod runner_wire;
pub mod sse;

pub use runner_wire::LiveTurnRequest;

use axum::middleware;
use dashmap::DashMap;
use saint_harness_ai::{ModelResolver, ProtocolRegistry};
use saint_harness_codemode::{PluginRegistry, QuickJsScriptHost};
use saint_harness_core::{
    EventStore, PermissionService, SessionInbox, SessionRunCoordinator, run_boot_recovery,
};
use saint_harness_edit::FileWriteCoordinator;
use saint_harness_lsp::{FastLintEngine, LazyLspManager};
use saint_harness_schema::{
    CodeOutliner, FastLintPort, FileWritePort, InteractionMode, LspPort, NoopPolicyGuard,
    PolicyGuard, RepoMapper, RuntimeMode, SandboxRunner, ScriptHost,
};
use saint_harness_shell::ShellSandboxRunner;
use saint_harness_treesitter::TreeSitterEngine;
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Composition root wiring `saint-harness-core` to all heavy leaf crates via `Arc<dyn Trait>` ports.
#[derive(Clone)]
pub struct AppState {
    pub store: EventStore,
    pub inbox: SessionInbox,
    pub coordinator: SessionRunCoordinator,
    pub perms: PermissionService,
    pub protocols: Arc<ProtocolRegistry>,
    pub model_resolver: Arc<ModelResolver>,
    pub file_writer: Arc<dyn FileWritePort>,
    pub outliner: Arc<dyn CodeOutliner>,
    pub repo_mapper: Arc<dyn RepoMapper>,
    pub lsp: Arc<dyn LspPort>,
    pub fast_lint: Arc<dyn FastLintPort>,
    pub shell_runner: Arc<dyn SandboxRunner>,
    pub script_host: Arc<dyn ScriptHost>,
    pub policy_guard: Arc<dyn PolicyGuard>,
    pub plugins: Arc<PluginRegistry>,
    pub workspace_root: PathBuf,
    pub home_dir: PathBuf,
    pub data_dir: PathBuf,
    pub advertise_url: String,
    pub socket_path: Option<String>,
    pub managed_mode: bool,
    pub started_at: Instant,
    // Strictly in-memory MCP specs (bearer tokens never touch SQLite).
    pub session_mcp_ram: Arc<DashMap<String, Vec<Value>>>,
    pub session_snapshot_disabled: Arc<DashMap<String, bool>>,
    pub session_modes: Arc<DashMap<String, (RuntimeMode, InteractionMode)>>,
}

impl AppState {
    pub async fn bootstrap(
        workspace_root: PathBuf,
        home_dir: PathBuf,
        data_dir: PathBuf,
        advertise_url: String,
        socket_path: Option<String>,
        managed_mode: bool,
    ) -> Result<Self, String> {
        let started_at = Instant::now();
        std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
        let db_path = data_dir.join("saint-harness.sqlite");
        let store = EventStore::open(db_path)?;
        let _ = run_boot_recovery(&store).await?;

        let inbox = SessionInbox::new(store.clone());
        let coordinator = SessionRunCoordinator::new();
        let perms = PermissionService::new(store.clone());
        let ts = Arc::new(TreeSitterEngine);
        let outliner: Arc<dyn CodeOutliner> = ts.clone();
        let repo_mapper: Arc<dyn RepoMapper> = ts;
        let fast_lint: Arc<dyn FastLintPort> = Arc::new(FastLintEngine);
        let lsp: Arc<dyn LspPort> = Arc::new(LazyLspManager::new(workspace_root.clone()));
        let file_writer: Arc<dyn FileWritePort> = Arc::new(FileWriteCoordinator::new(
            Some(outliner.clone()),
            Some(fast_lint.clone()),
            Some(lsp.clone()),
        ));
        let shell_runner: Arc<dyn SandboxRunner> = Arc::new(ShellSandboxRunner::default());
        let script_host: Arc<dyn ScriptHost> = Arc::new(QuickJsScriptHost::default());
        let policy_guard: Arc<dyn PolicyGuard> = Arc::new(NoopPolicyGuard);
        let plugin_dirs = vec![
            home_dir.join(".saint-harness").join("plugins"),
            workspace_root.join(".saint-harness").join("plugins"),
        ];
        let plugins = Arc::new(PluginRegistry::discover_dirs(&plugin_dirs));
        let model_resolver = Arc::new(ModelResolver::new(Some(data_dir.join("catalog"))));

        // Keep `saint-harness-token` linked cleanly at composition root.
        let _ = saint_harness_token::OUTPUT_TOKEN_MIN;

        Ok(Self {
            store,
            inbox,
            coordinator,
            perms,
            protocols: Arc::new(ProtocolRegistry::default()),
            model_resolver,
            file_writer,
            outliner,
            repo_mapper,
            lsp,
            fast_lint,
            shell_runner,
            script_host,
            policy_guard,
            plugins,
            workspace_root,
            home_dir,
            data_dir,
            advertise_url,
            socket_path,
            managed_mode,
            started_at,
            session_mcp_ram: Arc::new(DashMap::new()),
            session_snapshot_disabled: Arc::new(DashMap::new()),
            session_modes: Arc::new(DashMap::new()),
        })
    }

    pub fn started_elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

pub fn build_app(state: AppState, auth_token: Option<String>) -> axum::Router {
    routes::build_router(state).layer(middleware::from_fn(move |req, next| {
        auth::bearer_auth_middleware(auth_token.clone(), req, next)
    }))
}

pub async fn serve_tcp(
    addr: SocketAddr,
    state: AppState,
    auth_token: Option<String>,
) -> Result<(SocketAddr, tokio::task::JoinHandle<Result<(), String>>), String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("failed to bind {addr}: {e}"))?;
    let local_addr = listener.local_addr().map_err(|e| e.to_string())?;
    let mut state = state;
    state.advertise_url = format!("http://{local_addr}");
    let app = build_app(state, auth_token);
    let handle =
        tokio::spawn(async move { axum::serve(listener, app).await.map_err(|e| e.to_string()) });
    Ok((local_addr, handle))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_protocol::{
        CreateSessionRequest, CreateSessionResponse, HealthResponse, ModelsCatalogResponse,
        PromptRequest,
    };

    #[tokio::test]
    async fn server_http_endpoints_and_bearer_auth_work_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let state = AppState::bootstrap(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
            tmp.path().join("data"),
            "http://127.0.0.1:0".into(),
            None,
            false,
        )
        .await
        .unwrap();

        let (addr, _handle) = serve_tcp(
            "127.0.0.1:0".parse().unwrap(),
            state,
            Some("test-token-123".to_string()),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let base = format!("http://{addr}");

        // /health is reachable without auth
        let health: HealthResponse = client
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(health.ready);

        // Unauthorized request to /models returns 401
        let unauth = client.get(format!("{base}/models")).send().await.unwrap();
        assert_eq!(unauth.status().as_u16(), 401);

        // Authorized request succeeds
        let models: ModelsCatalogResponse = client
            .get(format!("{base}/models"))
            .bearer_auth("test-token-123")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(models.hardcoded_provider_versions, 0);
        assert_eq!(models.providers.len(), 2);

        // Create session + send prompt + fork
        let created: CreateSessionResponse = client
            .post(format!("{base}/sessions"))
            .bearer_auth("test-token-123")
            .json(&CreateSessionRequest {
                cwd: tmp.path().to_string_lossy().into_owned(),
                additional_directories: vec![],
                model_profile: "qwen-3.8-27b".into(),
                interaction_mode: Default::default(),
                runtime_mode: Default::default(),
                sandbox_mode: None,
                disable_internal_git_snapshot: true,
                title: Some("fix/wal-lock".into()),
                mcp_servers: vec![serde_json::json!({"name":"saint","bearer":"ram-only"})],
            })
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let prompt_resp = client
            .post(format!("{base}/sessions/{}/prompt", created.session_id))
            .bearer_auth("test-token-123")
            .json(&PromptRequest {
                text: "Verify WAL writer".into(),
                delivery: "queue".into(),
                client_msg_id: Some("cmsg_1".into()),
                profile_override: None,
                model_override: None,
                interaction_mode: None,
                runtime_mode: None,
            })
            .send()
            .await
            .unwrap();
        assert!(prompt_resp.status().is_success());

        let fork_resp: serde_json::Value = client
            .post(format!("{base}/sessions/{}/fork", created.session_id))
            .bearer_auth("test-token-123")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            fork_resp
                .get("session_id")
                .and_then(|v| v.as_str())
                .is_some()
        );
    }
}
