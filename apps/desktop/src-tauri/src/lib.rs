use reqwest::Client;
use saint_harness_protocol::{
    BtwRequest, BtwResponse, CreateSessionRequest, CreateSessionResponse, ForkRequest,
    HealthResponse, ModelsCatalogResponse, PermissionReplyRequest, PromptRequest,
    SessionSummaryDto, SseWireEvent, TreeNodeDto, UserInputReplyRequest, WireInteractionMode,
    WireRuntimeMode,
};
use saint_harness_schema::{
    PROVIDER_AUTH_PRESETS, RouteOverride, find_provider_preset, load_effective_settings,
    load_routes_with_overrides, load_settings_file, mask_secret_preview, resolve_env_value,
    resolve_os_home, save_user_settings, user_settings_path,
};
use saint_harness_server::{AppState, serve_tcp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::process::Command;
use tokio::sync::RwLock;
use ulid::Ulid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DesktopPersistedState {
    last_workspace: Option<String>,
}

struct DesktopRuntime {
    base_url: String,
    auth_token: String,
    http: Client,
    app_state: AppState,
    data_dir: PathBuf,
    workspace: RwLock<PathBuf>,
    _server_task: tokio::task::JoinHandle<Result<(), String>>,
    _event_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceInfo {
    path: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderAuthItem {
    id: String,
    label: String,
    route_key: String,
    env_key: String,
    protocol: String,
    base_url: String,
    default_base_url: String,
    models_endpoint: String,
    default_profile: String,
    is_local: bool,
    configured: bool,
    source: String,
    preview: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct AuthStatusResponse {
    settings_path: String,
    settings_exists: bool,
    default_profile: String,
    has_any_configured_key: bool,
    providers: Vec<ProviderAuthItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct SaveProviderAuthRequest {
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    set_default: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ProviderTestResult {
    ok: bool,
    status_code: Option<u16>,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
struct BootstrapResponse {
    version: String,
    workspace: WorkspaceInfo,
    default_profile: String,
    health: HealthResponse,
    sessions: Vec<SessionSummaryDto>,
    models: ModelsCatalogResponse,
    auth: AuthStatusResponse,
}

#[derive(Debug, Clone, Serialize)]
struct DiffFileEntry {
    path: String,
    status: String,
    added: usize,
    removed: usize,
    hunks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct WorkspaceDiffResponse {
    is_git_repo: bool,
    cwd: String,
    branch: String,
    total_added: usize,
    total_removed: usize,
    files: Vec<DiffFileEntry>,
}

fn desktop_state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("desktop-state.json")
}

fn load_desktop_state(data_dir: &Path) -> DesktopPersistedState {
    let Ok(raw) = std::fs::read_to_string(desktop_state_path(data_dir)) else {
        return DesktopPersistedState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_desktop_state(data_dir: &Path, state: &DesktopPersistedState) {
    let _ = std::fs::create_dir_all(data_dir);
    if let Ok(raw) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(desktop_state_path(data_dir), raw);
    }
}

fn workspace_info_from_path(path: &Path) -> WorkspaceInfo {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.display().to_string());
    WorkspaceInfo {
        path: path.display().to_string(),
        name,
    }
}

fn provider_display_label(id: &str) -> &'static str {
    match id {
        "anthropic" => "Anthropic (Claude)",
        "gemini" => "Google AI Studio (Gemini)",
        "dashscope" => "DashScope (Qwen Cloud)",
        "local" => "Local SGLang (127.0.0.1:30000)",
        "vllm" => "Local vLLM / Ollama (127.0.0.1:8000)",
        "openai" => "OpenAI-Compatible Endpoint",
        _ => "Custom Provider",
    }
}

fn build_auth_status(workspace: &Path) -> Result<AuthStatusResponse, String> {
    let settings_path = user_settings_path(None);
    let routes = load_routes_with_overrides(Some(workspace), None).map_err(|e| e.to_string())?;
    let effective = load_effective_settings(Some(workspace), None);
    let default_profile = effective
        .default_profile
        .clone()
        .unwrap_or_else(|| "qwen-3.8-27b".to_string());

    let mut has_any_configured_key = false;
    let mut providers = Vec::with_capacity(PROVIDER_AUTH_PRESETS.len());
    for preset in PROVIDER_AUTH_PRESETS {
        let active_route = routes.routes.get(preset.route_key);
        let active_base_url = active_route
            .map(|r| r.base_url.clone())
            .unwrap_or_else(|| preset.default_base_url.to_string());
        let active_env_key = active_route
            .map(|r| r.api_key_env.clone())
            .unwrap_or_else(|| preset.env_key.to_string());
        let active_models_ep = active_route
            .map(|r| r.models_endpoint.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| preset.default_models_endpoint.to_string());
        let is_local =
            active_base_url.contains("127.0.0.1") || active_base_url.contains("localhost");
        let resolved = resolve_env_value(&active_env_key, Some(workspace), None);
        let (configured, source, preview) = match resolved {
            Some((val, src)) => {
                if !is_local {
                    has_any_configured_key = true;
                }
                (true, src.to_string(), Some(mask_secret_preview(&val)))
            }
            None => (false, "unset".to_string(), None),
        };
        providers.push(ProviderAuthItem {
            id: preset.id.to_string(),
            label: provider_display_label(preset.id).to_string(),
            route_key: preset.route_key.to_string(),
            env_key: active_env_key,
            protocol: preset.protocol.to_string(),
            base_url: active_base_url,
            default_base_url: preset.default_base_url.to_string(),
            models_endpoint: active_models_ep,
            default_profile: preset.default_profile.to_string(),
            is_local,
            configured,
            source,
            preview,
        });
    }

    Ok(AuthStatusResponse {
        settings_path: settings_path.display().to_string(),
        settings_exists: settings_path.exists(),
        default_profile,
        has_any_configured_key,
        providers,
    })
}

async fn init_runtime(app_handle: AppHandle) -> Result<DesktopRuntime, String> {
    let home_dir = resolve_os_home();
    let data_dir = std::env::var("SAINT_HARNESS_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir.join(".saint-harness"));

    let persisted = load_desktop_state(&data_dir);
    let initial_ws = persisted
        .last_workspace
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| std::env::current_dir().ok().filter(|p| p.is_dir()))
        .unwrap_or_else(|| home_dir.clone());

    let auth_token = format!("sh_desk_{}", Ulid::new());
    let app_state = AppState::bootstrap(
        initial_ws.clone(),
        home_dir,
        data_dir.clone(),
        "http://127.0.0.1:0".to_string(),
        None,
        false,
    )
    .await?;

    let mut bus_rx = app_state.store.bus.subscribe();
    let event_handle = app_handle.clone();
    let event_task = tokio::spawn(async move {
        loop {
            match bus_rx.recv().await {
                Ok(env) => {
                    let wire = SseWireEvent {
                        stream_id: env.stream_id,
                        event_type: env.event.event_type_name().to_string(),
                        session_id: env.event.session_id().to_string(),
                        payload: serde_json::to_value(&env.event).unwrap_or_default(),
                    };
                    let _ = event_handle.emit("saint-event", wire);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let bind_addr: SocketAddr = "127.0.0.1:0"
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let (bound, server_task) =
        serve_tcp(bind_addr, app_state.clone(), Some(auth_token.clone())).await?;

    let http = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    Ok(DesktopRuntime {
        base_url: format!("http://{bound}"),
        auth_token,
        http,
        app_state,
        data_dir,
        workspace: RwLock::new(initial_ws),
        _server_task: server_task,
        _event_task: event_task,
    })
}

async fn api_get<T: serde::de::DeserializeOwned>(
    rt: &DesktopRuntime,
    path: &str,
) -> Result<T, String> {
    let resp = rt
        .http
        .get(format!("{}{path}", rt.base_url))
        .bearer_auth(&rt.auth_token)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {code}: {}", body.trim()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

async fn api_post<B: Serialize, T: serde::de::DeserializeOwned>(
    rt: &DesktopRuntime,
    path: &str,
    body: &B,
) -> Result<T, String> {
    let resp = rt
        .http
        .post(format!("{}{path}", rt.base_url))
        .bearer_auth(&rt.auth_token)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let code = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {code}: {}", txt.trim()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn desktop_bootstrap(
    rt: State<'_, Arc<DesktopRuntime>>,
) -> Result<BootstrapResponse, String> {
    let ws = rt.workspace.read().await.clone();
    let health: HealthResponse = api_get(&rt, "/health").await?;
    let models: ModelsCatalogResponse = api_get(&rt, "/models").await?;
    let auth = build_auth_status(&ws)?;
    let mut sessions: Vec<SessionSummaryDto> = api_get(&rt, "/sessions").await?;

    if sessions.is_empty() {
        let created: CreateSessionResponse = api_post(
            &rt,
            "/sessions",
            &CreateSessionRequest {
                cwd: ws.display().to_string(),
                additional_directories: Vec::new(),
                model_profile: auth.default_profile.clone(),
                interaction_mode: WireInteractionMode::Build,
                runtime_mode: WireRuntimeMode::ApprovalRequired,
                sandbox_mode: None,
                disable_internal_git_snapshot: false,
                title: Some("new session".to_string()),
                mcp_servers: Vec::new(),
            },
        )
        .await?;
        sessions.push(created.session);
    }

    Ok(BootstrapResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        workspace: workspace_info_from_path(&ws),
        default_profile: auth.default_profile.clone(),
        health,
        sessions,
        models,
        auth,
    })
}

#[tauri::command]
async fn list_sessions(
    rt: State<'_, Arc<DesktopRuntime>>,
) -> Result<Vec<SessionSummaryDto>, String> {
    api_get(&rt, "/sessions").await
}

#[tauri::command]
async fn get_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
) -> Result<SessionSummaryDto, String> {
    api_get(&rt, &format!("/sessions/{session_id}")).await
}

#[tauri::command]
async fn create_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    cwd: Option<String>,
    title: Option<String>,
    model_profile: Option<String>,
) -> Result<CreateSessionResponse, String> {
    let target_ws = if let Some(custom) = cwd.filter(|s| !s.trim().is_empty()) {
        let p = PathBuf::from(&custom);
        if !p.is_dir() {
            return Err(format!("directory does not exist: {custom}"));
        }
        *rt.workspace.write().await = p.clone();
        save_desktop_state(
            &rt.data_dir,
            &DesktopPersistedState {
                last_workspace: Some(p.display().to_string()),
            },
        );
        p
    } else {
        rt.workspace.read().await.clone()
    };

    let effective = load_effective_settings(Some(&target_ws), None);
    let profile = model_profile
        .filter(|s| !s.trim().is_empty())
        .or(effective.default_profile)
        .unwrap_or_else(|| "qwen-3.8-27b".to_string());

    api_post(
        &rt,
        "/sessions",
        &CreateSessionRequest {
            cwd: target_ws.display().to_string(),
            additional_directories: Vec::new(),
            model_profile: profile,
            interaction_mode: WireInteractionMode::Build,
            runtime_mode: WireRuntimeMode::ApprovalRequired,
            sandbox_mode: None,
            disable_internal_git_snapshot: false,
            title: title.or_else(|| Some("new session".to_string())),
            mcp_servers: Vec::new(),
        },
    )
    .await
}

#[tauri::command]
async fn delete_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
) -> Result<(), String> {
    let resp = rt
        .http
        .delete(format!("{}/sessions/{session_id}", rt.base_url))
        .bearer_auth(&rt.auth_token)
        .send()
        .await
        .map_err(|e| format!("delete failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    Ok(())
}

#[tauri::command]
async fn set_workspace(
    rt: State<'_, Arc<DesktopRuntime>>,
    path: String,
) -> Result<WorkspaceInfo, String> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("selected folder does not exist: {path}"));
    }
    *rt.workspace.write().await = p.clone();
    save_desktop_state(
        &rt.data_dir,
        &DesktopPersistedState {
            last_workspace: Some(p.display().to_string()),
        },
    );
    Ok(workspace_info_from_path(&p))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn send_prompt(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
    text: String,
    delivery: Option<String>,
    profile_override: Option<String>,
    model_override: Option<String>,
    interaction_mode: Option<WireInteractionMode>,
    runtime_mode: Option<WireRuntimeMode>,
) -> Result<Value, String> {
    api_post(
        &rt,
        &format!("/sessions/{session_id}/prompt"),
        &PromptRequest {
            text,
            delivery: delivery.unwrap_or_else(|| "queue".to_string()),
            client_msg_id: None,
            profile_override,
            model_override,
            interaction_mode,
            runtime_mode,
        },
    )
    .await
}

#[tauri::command]
async fn interrupt_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
) -> Result<Value, String> {
    api_post(
        &rt,
        &format!("/sessions/{session_id}/interrupt"),
        &Value::Null,
    )
    .await
}

#[tauri::command]
async fn reply_permission(
    rt: State<'_, Arc<DesktopRuntime>>,
    request_id: String,
    decision: String,
    pattern: Option<String>,
    feedback: Option<String>,
) -> Result<Value, String> {
    api_post(
        &rt,
        &format!("/permissions/{request_id}/reply"),
        &PermissionReplyRequest {
            decision,
            pattern,
            feedback,
        },
    )
    .await
}

#[tauri::command]
async fn reply_user_input(
    rt: State<'_, Arc<DesktopRuntime>>,
    request_id: String,
    answers: BTreeMap<String, Vec<String>>,
) -> Result<Value, String> {
    api_post(
        &rt,
        &format!("/user-inputs/{request_id}/reply"),
        &UserInputReplyRequest { answers },
    )
    .await
}

#[tauri::command]
async fn send_btw(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
    question: String,
) -> Result<BtwResponse, String> {
    api_post(
        &rt,
        &format!("/sessions/{session_id}/btw"),
        &BtwRequest { question },
    )
    .await
}

#[tauri::command]
async fn compact_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
) -> Result<Value, String> {
    api_post(
        &rt,
        &format!("/sessions/{session_id}/compact"),
        &Value::Null,
    )
    .await
}

#[tauri::command]
async fn get_session_tree(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
) -> Result<Vec<TreeNodeDto>, String> {
    api_get(&rt, &format!("/sessions/{session_id}/tree")).await
}

#[tauri::command]
async fn fork_session(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: String,
    through_msg_id: Option<String>,
) -> Result<CreateSessionResponse, String> {
    let forked: Value = api_post(
        &rt,
        &format!("/sessions/{session_id}/fork"),
        &ForkRequest { through_msg_id },
    )
    .await?;
    let new_id = forked["session_id"]
        .as_str()
        .ok_or_else(|| "fork returned no session_id".to_string())?
        .to_string();
    let session: SessionSummaryDto = api_get(&rt, &format!("/sessions/{new_id}")).await?;
    Ok(CreateSessionResponse {
        session_id: new_id,
        session,
    })
}

#[tauri::command]
async fn get_models(rt: State<'_, Arc<DesktopRuntime>>) -> Result<ModelsCatalogResponse, String> {
    api_get(&rt, "/models").await
}

#[tauri::command]
async fn get_auth_status(rt: State<'_, Arc<DesktopRuntime>>) -> Result<AuthStatusResponse, String> {
    let ws = rt.workspace.read().await.clone();
    build_auth_status(&ws)
}

#[tauri::command]
async fn save_provider_auth(
    rt: State<'_, Arc<DesktopRuntime>>,
    req: SaveProviderAuthRequest,
) -> Result<AuthStatusResponse, String> {
    let preset = find_provider_preset(&req.provider)
        .ok_or_else(|| format!("unknown provider preset `{}`", req.provider))?;
    let settings_path = user_settings_path(None);
    let mut settings = load_settings_file(&settings_path);

    if let Some(key) = req.api_key.map(|s| s.trim().to_string())
        && !key.is_empty()
    {
        settings.env.insert(preset.env_key.to_string(), key);
    }

    if let Some(url) = req.base_url.map(|s| s.trim().to_string()) {
        if url.is_empty() || url == preset.default_base_url {
            settings.routes.remove(preset.route_key);
        } else {
            settings.routes.insert(
                preset.route_key.to_string(),
                RouteOverride {
                    protocol: Some(preset.protocol.to_string()),
                    base_url: Some(url),
                    models_endpoint: Some(preset.default_models_endpoint.to_string()),
                    api_key_env: Some(preset.env_key.to_string()),
                },
            );
        }
    }

    if req.set_default {
        settings.default_profile = Some(preset.default_profile.to_string());
    }

    save_user_settings(&settings, None).map_err(|e| e.to_string())?;
    let ws = rt.workspace.read().await.clone();
    build_auth_status(&ws)
}

#[tauri::command]
async fn remove_provider_auth(
    rt: State<'_, Arc<DesktopRuntime>>,
    provider: String,
) -> Result<AuthStatusResponse, String> {
    let preset = find_provider_preset(&provider)
        .ok_or_else(|| format!("unknown provider preset `{provider}`"))?;
    let settings_path = user_settings_path(None);
    let mut settings = load_settings_file(&settings_path);
    settings.env.remove(preset.env_key);
    settings.routes.remove(preset.route_key);
    save_user_settings(&settings, None).map_err(|e| e.to_string())?;
    let ws = rt.workspace.read().await.clone();
    build_auth_status(&ws)
}

#[tauri::command]
async fn test_provider_connection(
    rt: State<'_, Arc<DesktopRuntime>>,
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
) -> Result<ProviderTestResult, String> {
    let preset = find_provider_preset(&provider)
        .ok_or_else(|| format!("unknown provider preset `{provider}`"))?;
    let ws = rt.workspace.read().await.clone();
    let routes = load_routes_with_overrides(Some(&ws), None).map_err(|e| e.to_string())?;
    let route = routes.routes.get(preset.route_key);

    let effective_base = base_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| route.map(|r| r.base_url.clone()))
        .unwrap_or_else(|| preset.default_base_url.to_string());
    let models_ep = route
        .map(|r| r.models_endpoint.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| preset.default_models_endpoint.to_string());
    let effective_key = api_key
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| resolve_env_value(preset.env_key, Some(&ws), None).map(|(v, _)| v))
        .unwrap_or_default();

    let url = format!("{}{}", effective_base.trim_end_matches('/'), models_ep);
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    match preset.protocol {
        "anthropic_messages" => {
            req = req.header("anthropic-version", "2023-06-01");
            if !effective_key.is_empty() {
                req = req.header("x-api-key", &effective_key);
            }
        }
        "gemini_generate_content" => {
            if !effective_key.is_empty() {
                req = req.header("x-goog-api-key", &effective_key);
            }
        }
        _ => {
            if !effective_key.is_empty() {
                req = req.bearer_auth(&effective_key);
            }
        }
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                Ok(ProviderTestResult {
                    ok: true,
                    status_code: Some(status.as_u16()),
                    message: format!("Connected ({}) at {url}", status.as_u16()),
                })
            } else {
                let body = resp.text().await.unwrap_or_default();
                let excerpt: String = body.chars().take(180).collect();
                Ok(ProviderTestResult {
                    ok: false,
                    status_code: Some(status.as_u16()),
                    message: format!("HTTP {} from {url}: {}", status.as_u16(), excerpt.trim()),
                })
            }
        }
        Err(e) => Ok(ProviderTestResult {
            ok: false,
            status_code: None,
            message: format!("Connection failed to {url}: {e}"),
        }),
    }
}

#[tauri::command]
async fn get_workspace_diff(
    rt: State<'_, Arc<DesktopRuntime>>,
    session_id: Option<String>,
) -> Result<WorkspaceDiffResponse, String> {
    let cwd = if let Some(ref sid) = session_id {
        rt.app_state
            .store
            .reader
            .get_session(sid)
            .ok()
            .flatten()
            .map(|r| PathBuf::from(r.cwd))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| rt.app_state.workspace_root.clone())
    } else {
        rt.workspace.read().await.clone()
    };

    let cwd_str = cwd.display().to_string();
    let probe = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&cwd)
        .output()
        .await;

    let Ok(probe_out) = probe else {
        return Ok(WorkspaceDiffResponse {
            is_git_repo: false,
            cwd: cwd_str,
            branch: String::new(),
            total_added: 0,
            total_removed: 0,
            files: Vec::new(),
        });
    };
    if !probe_out.status.success() {
        return Ok(WorkspaceDiffResponse {
            is_git_repo: false,
            cwd: cwd_str,
            branch: String::new(),
            total_added: 0,
            total_removed: 0,
            files: Vec::new(),
        });
    }

    let branch_out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&cwd)
        .output()
        .await
        .ok();
    let branch = branch_out
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "HEAD".to_string());

    let status_out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&cwd)
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let status_text = String::from_utf8_lossy(&status_out.stdout);

    let diff_out = Command::new("git")
        .args(["diff", "--no-color", "-U3", "HEAD"])
        .current_dir(&cwd)
        .output()
        .await
        .ok();
    let diff_text = match diff_out {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => {
            let fallback = Command::new("git")
                .args(["diff", "--no-color", "-U3"])
                .current_dir(&cwd)
                .output()
                .await
                .map_err(|e| e.to_string())?;
            String::from_utf8_lossy(&fallback.stdout).into_owned()
        }
    };

    let mut files_map: BTreeMap<String, DiffFileEntry> = BTreeMap::new();
    for line in status_text.lines() {
        if line.len() < 4 {
            continue;
        }
        let code = line[..2].trim().to_string();
        let path = line[3..].trim().to_string();
        files_map.insert(
            path.clone(),
            DiffFileEntry {
                path,
                status: if code == "??" {
                    "untracked".to_string()
                } else {
                    code
                },
                added: 0,
                removed: 0,
                hunks: Vec::new(),
            },
        );
    }

    let mut current_path: Option<String> = None;
    for line in diff_text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some((_, b_part)) = rest.split_once(" b/") {
                let p = b_part.to_string();
                files_map.entry(p.clone()).or_insert_with(|| DiffFileEntry {
                    path: p.clone(),
                    status: "M".to_string(),
                    added: 0,
                    removed: 0,
                    hunks: Vec::new(),
                });
                current_path = Some(p);
            }
            continue;
        }
        if let Some(ref path) = current_path
            && let Some(entry) = files_map.get_mut(path)
            && (line.starts_with("@@")
                || line.starts_with('+') && !line.starts_with("+++")
                || line.starts_with('-') && !line.starts_with("---")
                || line.starts_with(' '))
        {
            if line.starts_with('+') && !line.starts_with("+++") {
                entry.added += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                entry.removed += 1;
            }
            if entry.hunks.len() < 300 {
                entry.hunks.push(line.to_string());
            }
        }
    }

    let files: Vec<DiffFileEntry> = files_map.into_values().collect();
    let total_added = files.iter().map(|f| f.added).sum();
    let total_removed = files.iter().map(|f| f.removed).sum();

    Ok(WorkspaceDiffResponse {
        is_git_repo: true,
        cwd: cwd_str,
        branch,
        total_added,
        total_removed,
        files,
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let runtime = tauri::async_runtime::block_on(init_runtime(handle))
                .map_err(std::io::Error::other)?;
            app.manage(Arc::new(runtime));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            desktop_bootstrap,
            list_sessions,
            get_session,
            create_session,
            delete_session,
            set_workspace,
            send_prompt,
            interrupt_session,
            reply_permission,
            reply_user_input,
            send_btw,
            compact_session,
            get_session_tree,
            fork_session,
            get_models,
            get_auth_status,
            save_provider_auth,
            remove_provider_auth,
            test_provider_connection,
            get_workspace_diff,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("error while running SaintHarness desktop: {e}");
            std::process::exit(1);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_auth_status_lists_all_provider_presets() {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let status = build_auth_status(&cwd);
        assert!(status.is_ok(), "build_auth_status failed: {status:?}");
        if let Ok(st) = status {
            assert_eq!(st.providers.len(), PROVIDER_AUTH_PRESETS.len());
            assert!(st.providers.iter().any(|p| p.id == "anthropic"));
            assert!(st.providers.iter().any(|p| p.id == "gemini"));
            assert!(st.providers.iter().any(|p| p.id == "dashscope"));
        }
    }

    #[test]
    fn desktop_ui_assets_pass_antislop_hygiene() {
        let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui");
        for name in ["index.html", "styles.css", "app.js"] {
            let text = std::fs::read_to_string(ui_dir.join(name))
                .unwrap_or_else(|e| panic!("failed reading {name}: {e}"));
            assert!(
                !text.contains('\u{2014}'),
                "{name} must not contain em dash (U+2014)"
            );
            assert!(
                !text.contains("●●●"),
                "{name} must not render fake window traffic lights"
            );
        }
    }
}
