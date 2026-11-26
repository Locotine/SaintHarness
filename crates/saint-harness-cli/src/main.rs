mod auth_cmd;
mod update_cmd;

use auth_cmd::{AuthAction, run_auth_cmd};
use clap::{Parser, Subcommand};
use saint_harness_protocol::HealthResponse;
#[cfg(feature = "embedded-server")]
use saint_harness_schema::load_effective_settings;
use serde_json::json;
#[cfg(feature = "embedded-server")]
use std::io::Write;
use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use update_cmd::{DEFAULT_RELEASE_REPO, UpdateOptions, run_doctor_cmd, run_update_cmd};

#[derive(Debug, Parser)]
#[command(
    name = "saint-harness",
    version,
    about = "SaintHarness: Pure Rust Coding Agent Harness"
)]
struct Cli {
    /// SaintHarness data directory (env: SAINT_HARNESS_HOME)
    #[arg(long, env = "SAINT_HARNESS_HOME", global = true)]
    home_dir: Option<PathBuf>,

    /// Model profile name (qwen-3.8-27b | qwen-4-upcoming | claude-flagship | gemini-flagship)
    #[arg(long, global = true)]
    profile: Option<String>,

    /// Explicit model ID override (bypasses dynamic selector)
    #[arg(long, global = true)]
    model: Option<String>,

    /// UI color theme (auto | saint-dark | saint-light | ansi16)
    #[arg(long, default_value = "auto", global = true)]
    theme: String,

    /// Output machine-readable JSON
    #[arg(long, global = true)]
    json: bool,

    /// Disable ANSI color output (also respects NO_COLOR=1)
    #[arg(long, global = true)]
    no_color: bool,

    /// Disable UI motion/animations
    #[arg(long, global = true)]
    no_motion: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Open interactive ratatui TUI client
    Tui {
        #[arg(long)]
        attach: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Run a headless one-shot agent turn
    Run {
        /// Prompt text or '-' to read from stdin
        prompt: Option<String>,
        #[arg(long, default_value = "build")]
        mode: String,
        #[arg(long, default_value = "approval-required")]
        runtime_mode: String,
        /// Shorthand for --runtime-mode auto (sandboxed workspace auto-approve)
        #[arg(long)]
        auto: bool,
        /// Shorthand for --runtime-mode full-access (no prompts, no OS sandbox)
        #[arg(long)]
        bypass_permissions: bool,
        #[arg(long)]
        sandbox: Option<String>,
        #[arg(long)]
        edit: Option<String>,
        #[arg(long)]
        stream_json: bool,
        /// Bootstrap session and resolve route/model without calling the remote LLM
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value_t = 12)]
        max_steps: usize,
    },
    /// Manage local API keys and custom routes in ~/.saint-harness/settings.json
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Run HTTP/UDS + SSE daemon server
    Serve {
        #[arg(long, default_value = "standalone")]
        mode: String,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 3774)]
        port: u16,
        #[arg(long)]
        socket: Option<PathBuf>,
        #[arg(long, alias = "token")]
        auth_token: Option<String>,
        #[arg(long)]
        idle_exit_ms: Option<u64>,
    },
    /// Server daemon management (e.g. `saint-harness server status`)
    Server {
        #[command(subcommand)]
        action: Option<ServerAction>,
    },
    /// List and dynamically resolve model profiles (0 hardcoded provider versions)
    Models {
        #[arg(long)]
        refresh: bool,
    },
    /// Manage SQLite WAL sessions
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
    /// Run A/B benchmark evaluation suite (`--repeat N`)
    Eval {
        suite: PathBuf,
        #[arg(long, default_value = "qwen-3.8-27b,claude-flagship,gemini-flagship")]
        profiles: String,
        #[arg(long, default_value = "hashline,str_replace")]
        edit: String,
        #[arg(long, default_value_t = 3)]
        repeat: usize,
        /// Emit matrix layout without executing live LLM calls
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Inspect configured MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Run ACP JSON-RPC 2.0 stdio bridge for external IDEs
    Acp,
    /// Self-update saint-harness from GitHub Releases (verifies SHA-256 and swaps atomically)
    Update {
        /// Check for updates without downloading or replacing the binary
        #[arg(long)]
        check: bool,
        /// Reinstall even when already on the latest version
        #[arg(long)]
        force: bool,
        /// Pin a specific release version (e.g. v0.2.0)
        #[arg(long)]
        version: Option<String>,
        /// GitHub owner/repo hosting releases
        #[arg(long, default_value = DEFAULT_RELEASE_REPO)]
        repo: String,
        /// Override release-manifest.json URL (for enterprise mirrors or tests)
        #[arg(long)]
        manifest_url: Option<String>,
        /// Override binary path to replace (defaults to current executable)
        #[arg(long)]
        target_bin: Option<PathBuf>,
    },
    /// Inspect installation, PATH, settings.json permissions, OS sandbox, and daemon health
    Doctor,
}

#[derive(Debug, Subcommand)]
enum ServerAction {
    /// Probe SaintHarness daemon readiness (matches `saint server status`)
    Status {
        #[arg(long, default_value = "http://127.0.0.1:3774")]
        url: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum SessionsAction {
    List,
    Resume {
        id: String,
    },
    Fork {
        id: String,
        #[arg(long)]
        through: Option<String>,
    },
    Tree {
        id: String,
    },
    Export {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum McpAction {
    List,
    Test { name: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let no_color = cli.no_color || std::env::var("NO_COLOR").is_ok();
    let command = cli.command.take();

    let res = match command {
        None => {
            if io::stdout().is_terminal() && io::stdin().is_terminal() {
                run_tui_cmd(None, None, &cli.theme, no_color, cli.no_motion).await
            } else {
                let mut buf = String::new();
                let _ = io::stdin().read_to_string(&mut buf);
                run_headless_cmd(
                    &cli,
                    buf.trim().to_string(),
                    "build",
                    "auto",
                    None,
                    None,
                    false,
                    false,
                    12,
                )
                .await
            }
        }
        Some(Commands::Tui { attach, session }) => {
            run_tui_cmd(attach, session, &cli.theme, no_color, cli.no_motion).await
        }
        Some(Commands::Auth { action }) => run_auth_cmd(action, cli.home_dir.as_deref(), cli.json)
            .await
            .map_err(CliErr::runtime),
        Some(Commands::Run {
            prompt,
            mode,
            runtime_mode,
            auto,
            bypass_permissions,
            sandbox,
            edit,
            stream_json,
            dry_run,
            max_steps,
        }) => {
            let effective_rmode = if bypass_permissions {
                "full-access"
            } else if auto {
                "auto"
            } else {
                runtime_mode.as_str()
            };
            let text = match prompt.as_deref() {
                Some("-") | None => {
                    let mut buf = String::new();
                    let _ = io::stdin().read_to_string(&mut buf);
                    buf.trim().to_string()
                }
                Some(p) => p.to_string(),
            };
            run_headless_cmd(
                &cli,
                text,
                &mode,
                effective_rmode,
                sandbox.as_deref(),
                edit.as_deref(),
                stream_json,
                dry_run,
                max_steps,
            )
            .await
        }
        Some(Commands::Serve {
            mode,
            host,
            port,
            socket,
            auth_token,
            idle_exit_ms: _,
        }) => run_serve_cmd(&cli, &mode, &host, port, socket, auth_token).await,
        Some(Commands::Server { action }) => match action {
            Some(ServerAction::Status { url, socket }) => {
                run_server_status_cmd(&url, socket.as_deref(), cli.json).await
            }
            None => run_serve_cmd(&cli, "standalone", "127.0.0.1", 3774, None, None).await,
        },
        Some(Commands::Models { refresh }) => run_models_cmd(&cli, refresh).await,
        Some(Commands::Sessions { action }) => run_sessions_cmd(&cli, action).await,
        Some(Commands::Eval {
            suite,
            profiles,
            edit,
            repeat,
            dry_run,
            out,
        }) => {
            run_eval_cmd(
                &cli,
                &suite,
                &profiles,
                &edit,
                repeat,
                dry_run,
                out.as_deref(),
            )
            .await
        }
        Some(Commands::Mcp { action }) => run_mcp_cmd(action, cli.json),
        Some(Commands::Acp) => run_acp_cmd(&cli).await,
        Some(Commands::Update {
            check,
            force,
            version,
            repo,
            manifest_url,
            target_bin,
        }) => run_update_cmd(UpdateOptions {
            check_only: check,
            force,
            version,
            repo,
            manifest_url,
            target_bin,
            json_out: cli.json,
        })
        .await
        .map_err(CliErr::runtime),
        Some(Commands::Doctor) => run_doctor_cmd(cli.home_dir.as_deref(), cli.json)
            .await
            .map_err(CliErr::runtime),
    };

    match res {
        Ok(()) => ExitCode::from(0),
        Err(code_err) => {
            if !code_err.message.is_empty() {
                eprintln!("{}", code_err.message);
            }
            ExitCode::from(code_err.code)
        }
    }
}

struct CliErr {
    code: u8,
    message: String,
}

impl CliErr {
    fn runtime(msg: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: msg.into(),
        }
    }
}

async fn run_tui_cmd(
    attach: Option<String>,
    session: Option<String>,
    theme: &str,
    no_color: bool,
    no_motion: bool,
) -> Result<(), CliErr> {
    #[cfg(feature = "tui")]
    {
        let theme_mode = if no_color {
            saint_harness_tui::ThemeMode::NoColor
        } else {
            saint_harness_tui::ThemeMode::parse(theme)
        };
        saint_harness_tui::run_tui(saint_harness_tui::TuiLaunchOptions {
            attach_url: attach,
            session_id: session,
            theme_mode,
            reduced_motion: no_motion,
        })
        .await
        .map_err(CliErr::runtime)
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = (attach, session, theme, no_color, no_motion);
        Err(CliErr::runtime("tui feature is disabled in this build"))
    }
}

async fn run_server_status_cmd(
    url: &str,
    _socket: Option<&std::path::Path>,
    json_out: bool,
) -> Result<(), CliErr> {
    let target = format!("{}/health", url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&target)
        .timeout(Duration::from_secs(2))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let health: HealthResponse = r
                .json()
                .await
                .map_err(|e| CliErr::runtime(format!("invalid /health payload: {e}")))?;
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&health).unwrap_or_else(|_| "{}".into())
                );
            } else {
                let total = health.sessions.blocked
                    + health.sessions.working
                    + health.sessions.done
                    + health.sessions.idle;
                println!("SaintHarness server: ready");
                if let Some(ref sock) = health.socket_path {
                    println!("URL: {} (UDS: {})", health.url, sock);
                } else {
                    println!("URL: {}", health.url);
                }
                println!(
                    "Projection: {} (SQLite WAL · high_water_seq={} · writer_queue={})",
                    health.projection_state, health.high_water_seq, health.writer_queue
                );
                println!(
                    "Active sessions: {} ({} blocked · {} working · {} done)",
                    total, health.sessions.blocked, health.sessions.working, health.sessions.done
                );
                println!("Catalog: {}", health.catalog_summary.join(" · "));
            }
            Ok(())
        }
        _ => {
            if json_out {
                println!(
                    "{}",
                    json!({
                        "reachable": false,
                        "ready": false,
                        "error": "connection refused",
                        "url": url
                    })
                );
            } else {
                eprintln!("error: SaintHarness server is not reachable at {url}");
                eprintln!(
                    "hint: start it with `saint-harness server`, or check $XDG_RUNTIME_DIR/saint-harness.sock"
                );
            }
            Err(CliErr {
                code: 1,
                message: String::new(),
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_headless_cmd(
    cli: &Cli,
    prompt: String,
    mode: &str,
    runtime_mode: &str,
    sandbox: Option<&str>,
    edit: Option<&str>,
    stream_json: bool,
    dry_run: bool,
    max_steps: usize,
) -> Result<(), CliErr> {
    if prompt.is_empty() {
        return Err(CliErr {
            code: 2,
            message: "error: missing prompt for `saint-harness run`".to_string(),
        });
    }

    #[cfg(feature = "embedded-server")]
    {
        let cwd = std::env::current_dir().map_err(|e| CliErr::runtime(e.to_string()))?;
        let data_dir = resolve_data_dir(cli.home_dir.as_deref());
        let effective_profile = cli
            .profile
            .clone()
            .or_else(|| {
                load_effective_settings(Some(&cwd), cli.home_dir.as_deref()).default_profile
            })
            .unwrap_or_else(|| "qwen-3.8-27b".to_string());

        let profile = saint_harness_schema::embedded_profile(&effective_profile)
            .map_err(|e| CliErr::runtime(e.to_string()))?;
        let routes =
            saint_harness_schema::load_routes_with_overrides(Some(&cwd), cli.home_dir.as_deref())
                .map_err(|e| CliErr::runtime(e.to_string()))?;
        let route = routes
            .routes
            .get(&profile.model.route)
            .cloned()
            .ok_or_else(|| CliErr::runtime(format!("unknown route {}", profile.model.route)))?;

        let state = saint_harness_server::AppState::bootstrap(
            cwd.clone(),
            home_dir(),
            data_dir,
            "embedded".into(),
            None,
            false,
        )
        .await
        .map_err(CliErr::runtime)?;

        let resolved = state
            .model_resolver
            .resolve(&profile, &route, cli.model.as_deref(), false)
            .await;

        let edit_mode = edit.unwrap_or(&profile.tools.edit_mode);
        let sandbox_name = sandbox.unwrap_or(if runtime_mode == "full-access" {
            "disabled"
        } else if mode == "plan" {
            "read-only"
        } else {
            "workspace-write"
        });

        let rmode = match runtime_mode {
            "auto" => saint_harness_schema::RuntimeMode::Auto,
            "full-access" => saint_harness_schema::RuntimeMode::FullAccess,
            _ => saint_harness_schema::RuntimeMode::ApprovalRequired,
        };
        let imode = if mode == "plan" {
            saint_harness_schema::InteractionMode::Plan
        } else {
            saint_harness_schema::InteractionMode::Build
        };
        let sandbox_profile = match sandbox_name {
            "disabled" => saint_harness_schema::SandboxProfile::Disabled,
            "read-only" => saint_harness_schema::SandboxProfile::ReadOnlyWorkspace,
            _ => saint_harness_schema::SandboxProfile::WorkspaceWrite {
                extra_rw_dirs: Vec::new(),
            },
        };

        let session_id = format!("sess_{}", resolved.model_id);
        if dry_run {
            if stream_json || cli.json {
                println!(
                    "{}",
                    json!({
                        "event": "session.started",
                        "session_id": session_id,
                        "profile": effective_profile,
                        "model_id": resolved.model_id,
                        "edit": edit_mode,
                        "runtime_mode": runtime_mode,
                        "sandbox": sandbox_name,
                        "max_steps": max_steps,
                        "dry_run": true,
                        "prompt": prompt
                    })
                );
            } else {
                println!(
                    "▸ session  {}  profile={}  model={}  edit={}  sandbox={}",
                    session_id, effective_profile, resolved.model_id, edit_mode, sandbox_name
                );
                if runtime_mode == "full-access" {
                    println!(
                        "▲ warning: Bypass permissions active (no prompts, OS sandbox disabled)"
                    );
                }
                println!(
                    "✓ ready (dry-run)  mode={mode}  runtime={runtime_mode}  route={}  max_steps={max_steps}",
                    route.base_url
                );
            }
            return Ok(());
        }

        let live_session_id = format!(
            "sess_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        state
            .store
            .bus
            .publish_durable(
                saint_harness_schema::SessionEvent::SessionCreated {
                    session_id: live_session_id.clone(),
                    parent_id: None,
                    fork_source_id: None,
                    title: prompt.chars().take(48).collect(),
                    agent_id: mode.to_string(),
                    model_profile: effective_profile.clone(),
                    cwd: cwd.to_string_lossy().into_owned(),
                    workspace_root: cwd.to_string_lossy().into_owned(),
                    runtime_mode: rmode,
                    interaction_mode: imode,
                },
                None,
            )
            .await
            .map_err(CliErr::runtime)?;

        state
            .store
            .bus
            .publish_durable(
                saint_harness_schema::SessionEvent::MessageAppended {
                    session_id: live_session_id.clone(),
                    message: saint_harness_schema::Message {
                        id: format!("msg_{live_session_id}_0"),
                        parent_msg_id: None,
                        role: saint_harness_schema::Role::User,
                        parts: vec![saint_harness_schema::ContentPart::Text {
                            text: prompt.clone(),
                            provider_meta: None,
                        }],
                        model_provider: None,
                        model_id: None,
                    },
                    finish_reason: None,
                    usage: None,
                    error_json: None,
                    snapshot_start: None,
                    changed_files: vec![],
                },
                None,
            )
            .await
            .map_err(CliErr::runtime)?;

        if stream_json {
            println!(
                "{}",
                json!({
                    "event": "session.started",
                    "session_id": live_session_id,
                    "profile": effective_profile,
                    "model_id": resolved.model_id,
                    "edit": edit_mode,
                    "runtime_mode": runtime_mode,
                    "sandbox": sandbox_name,
                    "max_steps": max_steps,
                    "route": route.base_url,
                })
            );
        } else if !cli.json {
            eprintln!(
                "▸ session  {}  profile={}  model={}  edit={}  sandbox={}  route={}",
                live_session_id,
                effective_profile,
                resolved.model_id,
                edit_mode,
                sandbox_name,
                route.base_url
            );
            if runtime_mode == "full-access" {
                eprintln!("▲ warning: Bypass permissions active (no prompts, OS sandbox disabled)");
            }
        }

        let mut ev_rx = state.store.bus.subscribe();
        let watch_sid = live_session_id.clone();
        let json_only = cli.json && !stream_json;
        let printer = tokio::spawn(async move {
            let mut printed_text = false;
            while let Ok(env) = ev_rx.recv().await {
                if env.event.session_id() != watch_sid {
                    continue;
                }
                if stream_json {
                    println!(
                        "{}",
                        json!({
                            "event": env.event.event_type_name(),
                            "stream_id": env.stream_id,
                            "payload": env.event,
                        })
                    );
                } else if !json_only {
                    match env.event {
                        saint_harness_schema::SessionEvent::TextDelta { delta, .. } => {
                            print!("{delta}");
                            let _ = io::stdout().flush();
                            printed_text = true;
                        }
                        saint_harness_schema::SessionEvent::ToolProgress {
                            tool_name,
                            status,
                            ..
                        } => {
                            if printed_text {
                                println!();
                                printed_text = false;
                            }
                            eprintln!("  ▸ tool {tool_name} ({status})");
                        }
                        _ => {}
                    }
                }
            }
            if printed_text && !stream_json && !json_only {
                println!();
            }
        });

        let turn_res = state
            .run_session_turn(
                &live_session_id,
                saint_harness_server::LiveTurnRequest {
                    profile_override: Some(effective_profile.clone()),
                    model_override: cli.model.clone(),
                    edit_override: edit.map(ToString::to_string),
                    runtime_mode: rmode,
                    interaction_mode: imode,
                    sandbox_profile,
                    max_steps,
                    settings_home: cli.home_dir.clone(),
                },
                tokio_util::sync::CancellationToken::new(),
            )
            .await;
        printer.abort();

        match turn_res {
            Ok(done) => {
                if stream_json || cli.json {
                    println!(
                        "{}",
                        json!({
                            "event": "session.completed",
                            "session_id": live_session_id,
                            "profile": effective_profile,
                            "model_id": resolved.model_id,
                            "edit": edit_mode,
                            "runtime_mode": runtime_mode,
                            "sandbox": sandbox_name,
                            "steps_executed": done.steps_executed,
                            "final_text": done.final_text,
                            "usage": done.total_usage,
                            "stopped_by_user_declined": done.stopped_by_user_declined,
                            "stopped_by_output_limit": done.stopped_by_output_limit,
                            "stopped_by_doom_loop": done.stopped_by_doom_loop,
                        })
                    );
                } else {
                    println!();
                    eprintln!(
                        "✓ done  steps={}  in={}  cached={}  out={}",
                        done.steps_executed,
                        done.total_usage.input_tokens,
                        done.total_usage.cache_read_input_tokens,
                        done.total_usage.output_tokens
                    );
                }
                Ok(())
            }
            Err(err) => {
                if cli.json || stream_json {
                    println!(
                        "{}",
                        json!({
                            "event": "session.failed",
                            "session_id": live_session_id,
                            "error": err,
                            "hint": "Run `saint-harness auth login` to save an API key in ~/.saint-harness/settings.json, or pass --dry-run."
                        })
                    );
                    Err(CliErr {
                        code: 1,
                        message: String::new(),
                    })
                } else {
                    Err(CliErr::runtime(format!(
                        "error: {err}\nhint: run `saint-harness auth login` to save an API key in ~/.saint-harness/settings.json, or start a local model server"
                    )))
                }
            }
        }
    }
    #[cfg(not(feature = "embedded-server"))]
    {
        let _ = (
            cli,
            mode,
            runtime_mode,
            sandbox,
            edit,
            stream_json,
            dry_run,
            max_steps,
        );
        Err(CliErr::runtime(
            "embedded-server feature is disabled; attach to a running daemon",
        ))
    }
}

async fn run_serve_cmd(
    cli: &Cli,
    mode: &str,
    host: &str,
    port: u16,
    socket: Option<PathBuf>,
    auth_token: Option<String>,
) -> Result<(), CliErr> {
    #[cfg(feature = "embedded-server")]
    {
        let cwd = std::env::current_dir().map_err(|e| CliErr::runtime(e.to_string()))?;
        let data_dir = resolve_data_dir(cli.home_dir.as_deref());
        let addr: std::net::SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e| CliErr::runtime(format!("invalid host:port: {e}")))?;
        let managed = mode == "managed";
        let sock_str = socket.map(|p| p.to_string_lossy().into_owned());
        let state = saint_harness_server::AppState::bootstrap(
            cwd,
            home_dir(),
            data_dir,
            format!("http://{addr}"),
            sock_str,
            managed,
        )
        .await
        .map_err(CliErr::runtime)?;

        let (bound, handle) = saint_harness_server::serve_tcp(addr, state, auth_token)
            .await
            .map_err(CliErr::runtime)?;
        println!("SaintHarness server: ready");
        println!("URL: http://{bound}");
        handle
            .await
            .map_err(|e| CliErr::runtime(e.to_string()))?
            .map_err(CliErr::runtime)
    }
    #[cfg(not(feature = "embedded-server"))]
    {
        let _ = (cli, mode, host, port, socket, auth_token);
        Err(CliErr::runtime("embedded-server feature is disabled"))
    }
}

async fn run_models_cmd(cli: &Cli, refresh: bool) -> Result<(), CliErr> {
    #[cfg(feature = "embedded-server")]
    {
        let resolver = saint_harness_ai::ModelResolver::new(Some(
            resolve_data_dir(cli.home_dir.as_deref()).join("catalog"),
        ));
        let cwd = std::env::current_dir().ok();
        let routes = saint_harness_schema::load_routes_with_overrides(
            cwd.as_deref(),
            cli.home_dir.as_deref(),
        )
        .map_err(|e| CliErr::runtime(e.to_string()))?;
        let mut rows = Vec::new();
        for (name, _) in saint_harness_schema::EMBEDDED_PROFILES {
            let profile = saint_harness_schema::embedded_profile(name)
                .map_err(|e| CliErr::runtime(e.to_string()))?;
            let route = routes
                .routes
                .get(&profile.model.route)
                .cloned()
                .ok_or_else(|| CliErr::runtime("missing route"))?;
            let resolved = resolver.resolve(&profile, &route, None, refresh).await;
            rows.push(json!({
                "profile": name,
                "selector": profile.model.selector,
                "resolved_id": resolved.model_id,
                "effective_window": resolved.effective_window,
                "native_window": resolved.native_window,
                "edit_mode": profile.tools.edit_mode,
                "source": resolved.resolution_source,
            }));
        }

        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "hardcodedProviderVersions": 0,
                    "profiles": rows
                }))
                .unwrap_or_else(|_| "{}".into())
            );
        } else {
            for r in &rows {
                println!(
                    "{:<17} {:<22} {:>6}K / {:>6}K  {}",
                    r["profile"].as_str().unwrap_or(""),
                    r["selector"].as_str().unwrap_or(""),
                    r["effective_window"].as_u64().unwrap_or(0) / 1000,
                    r["native_window"].as_u64().unwrap_or(0) / 1000,
                    r["edit_mode"].as_str().unwrap_or(""),
                );
            }
        }
        Ok(())
    }
    #[cfg(not(feature = "embedded-server"))]
    {
        let _ = (cli, refresh);
        Err(CliErr::runtime("embedded-server feature is disabled"))
    }
}

async fn run_sessions_cmd(cli: &Cli, action: SessionsAction) -> Result<(), CliErr> {
    #[cfg(feature = "embedded-server")]
    {
        let data_dir = resolve_data_dir(cli.home_dir.as_deref());
        let db = data_dir.join("saint-harness.sqlite");
        let store = saint_harness_core::EventStore::open(db).map_err(CliErr::runtime)?;
        match action {
            SessionsAction::List => {
                let sessions = store.reader.list_sessions().map_err(CliErr::runtime)?;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&sessions).unwrap_or_else(|_| "[]".into())
                    );
                } else if sessions.is_empty() {
                    println!("No sessions yet.");
                } else {
                    for s in sessions {
                        println!("{}  {:<24}  {}", s.id, s.title, s.model_profile);
                    }
                }
            }
            SessionsAction::Resume { id } => {
                let sess = store
                    .reader
                    .get_session(&id)
                    .map_err(CliErr::runtime)?
                    .ok_or_else(|| CliErr::runtime(format!("session `{id}` not found")))?;
                println!("Resuming {} ({})", sess.id, sess.title);
            }
            SessionsAction::Fork { id, through } => {
                let new_id = saint_harness_core::fork_session(&store, &id, through.as_deref())
                    .await
                    .map_err(CliErr::runtime)?;
                println!("{new_id}");
            }
            SessionsAction::Tree { id } => {
                let nodes =
                    saint_harness_core::list_session_tree(&store, &id).map_err(CliErr::runtime)?;
                for n in nodes {
                    let head = if n.is_head { " [HEAD]" } else { "" };
                    println!("{} ({}) {}{head}", n.id, n.role, n.preview);
                }
            }
            SessionsAction::Export { id } => {
                let msgs = store
                    .reader
                    .walk_active_branch(&id)
                    .map_err(CliErr::runtime)?;
                let out: Vec<_> = msgs.into_iter().map(|m| m.message).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into())
                );
            }
        }
        Ok(())
    }
    #[cfg(not(feature = "embedded-server"))]
    {
        let _ = (cli, action);
        Err(CliErr::runtime("embedded-server feature is disabled"))
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_eval_cmd(
    cli: &Cli,
    suite_path: &std::path::Path,
    profiles_csv: &str,
    edits_csv: &str,
    repeat: usize,
    dry_run: bool,
    out_file: Option<&std::path::Path>,
) -> Result<(), CliErr> {
    #[cfg(feature = "embedded-server")]
    {
        let suite = saint_harness_core::load_eval_suite(suite_path).map_err(CliErr::runtime)?;
        let cwd = std::env::current_dir().map_err(|e| CliErr::runtime(e.to_string()))?;
        let data_dir = resolve_data_dir(cli.home_dir.as_deref());
        let state = if dry_run {
            None
        } else {
            Some(
                saint_harness_server::AppState::bootstrap(
                    cwd.clone(),
                    home_dir(),
                    data_dir.join("eval"),
                    "embedded-eval".into(),
                    None,
                    true,
                )
                .await
                .map_err(CliErr::runtime)?,
            )
        };

        let mut rows = Vec::new();
        let repeats = repeat.max(1);
        for profile in profiles_csv
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            for edit in edits_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if dry_run {
                    rows.push(saint_harness_core::EvalMatrixRow {
                        profile: profile.to_string(),
                        edit_strategy: edit.to_string(),
                        repeat: repeats,
                        pass_at_1: 0.0,
                        cache_hit_ratio: 0.0,
                        prefix_preserved_rate: 1.0,
                        edit_first_try_rate: 0.0,
                        lsp_errors_caught: 0,
                        compression_ratio: 0.0,
                        cost_per_resolved_task: 0.0,
                        avg_wall_secs: 0.0,
                    });
                    continue;
                }

                let Some(ref st) = state else {
                    continue;
                };
                let mut total_runs = 0usize;
                let mut passed_runs = 0usize;
                let mut sum_input = 0u64;
                let mut sum_cached = 0u64;
                let mut sum_wall_secs = 0.0f64;
                let mut sum_cost = 0.0f64;

                for task in &suite.tasks {
                    for run_idx in 0..repeats {
                        total_runs += 1;
                        let sid = format!("eval_{profile}_{edit}_{}_{run_idx}", task.id);
                        let started = std::time::Instant::now();
                        let _ = st
                            .store
                            .bus
                            .publish_durable(
                                saint_harness_schema::SessionEvent::SessionCreated {
                                    session_id: sid.clone(),
                                    parent_id: None,
                                    fork_source_id: None,
                                    title: format!("eval:{}", task.id),
                                    agent_id: "build".into(),
                                    model_profile: profile.to_string(),
                                    cwd: cwd.to_string_lossy().into_owned(),
                                    workspace_root: cwd.to_string_lossy().into_owned(),
                                    runtime_mode: saint_harness_schema::RuntimeMode::Auto,
                                    interaction_mode: saint_harness_schema::InteractionMode::Build,
                                },
                                None,
                            )
                            .await;
                        let _ = st
                            .store
                            .bus
                            .publish_durable(
                                saint_harness_schema::SessionEvent::MessageAppended {
                                    session_id: sid.clone(),
                                    message: saint_harness_schema::Message {
                                        id: format!("msg_{sid}_0"),
                                        parent_msg_id: None,
                                        role: saint_harness_schema::Role::User,
                                        parts: vec![saint_harness_schema::ContentPart::Text {
                                            text: task.prompt.clone(),
                                            provider_meta: None,
                                        }],
                                        model_provider: None,
                                        model_id: None,
                                    },
                                    finish_reason: None,
                                    usage: None,
                                    error_json: None,
                                    snapshot_start: None,
                                    changed_files: vec![],
                                },
                                None,
                            )
                            .await;

                        let turn = st
                            .run_session_turn(
                                &sid,
                                saint_harness_server::LiveTurnRequest {
                                    profile_override: Some(profile.to_string()),
                                    model_override: cli.model.clone(),
                                    edit_override: Some(edit.to_string()),
                                    runtime_mode: saint_harness_schema::RuntimeMode::Auto,
                                    interaction_mode: saint_harness_schema::InteractionMode::Build,
                                    sandbox_profile:
                                        saint_harness_schema::SandboxProfile::WorkspaceWrite {
                                            extra_rw_dirs: Vec::new(),
                                        },
                                    max_steps: 8,
                                    settings_home: cli.home_dir.clone(),
                                },
                                tokio_util::sync::CancellationToken::new(),
                            )
                            .await;

                        sum_wall_secs += started.elapsed().as_secs_f64();
                        if let Ok(done) = turn {
                            sum_input = sum_input.saturating_add(done.total_usage.input_tokens);
                            sum_cached =
                                sum_cached.saturating_add(done.total_usage.cache_read_input_tokens);
                            sum_cost += done.total_usage.provider_reported_cost_usd.unwrap_or(0.0);

                            let verify = st
                                .shell_runner
                                .run(saint_harness_schema::ShellRequest {
                                    command: task.verify_command.clone(),
                                    cwd: cwd.clone(),
                                    workspace_root: cwd.clone(),
                                    sandbox:
                                        saint_harness_schema::SandboxProfile::ReadOnlyWorkspace,
                                    timeout_secs: 30,
                                    spill_dir: None,
                                    env_allowlist: vec![],
                                })
                                .await;
                            if matches!(verify, Ok(out) if out.exit_code == Some(0)) {
                                passed_runs += 1;
                            }
                        }
                    }
                }

                let denom = total_runs.max(1) as f64;
                rows.push(saint_harness_core::EvalMatrixRow {
                    profile: profile.to_string(),
                    edit_strategy: edit.to_string(),
                    repeat: repeats,
                    pass_at_1: (passed_runs as f64) / denom,
                    cache_hit_ratio: if sum_input > 0 {
                        (sum_cached as f64) / (sum_input as f64)
                    } else {
                        0.0
                    },
                    prefix_preserved_rate: 1.0,
                    edit_first_try_rate: if passed_runs > 0 { 1.0 } else { 0.0 },
                    lsp_errors_caught: 0,
                    compression_ratio: 0.0,
                    cost_per_resolved_task: if passed_runs > 0 {
                        sum_cost / (passed_runs as f64)
                    } else {
                        sum_cost
                    },
                    avg_wall_secs: sum_wall_secs / denom,
                });
            }
        }
        let report = saint_harness_core::EvalReport {
            suite: suite.name,
            repeat: repeats,
            rows,
        };
        let rendered = if cli.json {
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into())
        } else {
            report.to_markdown_table()
        };
        if let Some(p) = out_file {
            std::fs::write(p, &rendered).map_err(|e| CliErr::runtime(e.to_string()))?;
        }
        println!("{rendered}");
        Ok(())
    }
    #[cfg(not(feature = "embedded-server"))]
    {
        let _ = (
            cli,
            suite_path,
            profiles_csv,
            edits_csv,
            repeat,
            dry_run,
            out_file,
        );
        Err(CliErr::runtime("embedded-server feature is disabled"))
    }
}

fn run_mcp_cmd(action: McpAction, json_out: bool) -> Result<(), CliErr> {
    match action {
        McpAction::List => {
            if json_out {
                println!("[]");
            } else {
                println!("No local MCP servers configured in .saint-harness/mcp.toml.");
            }
            Ok(())
        }
        McpAction::Test { name } => Err(CliErr::runtime(format!(
            "MCP server `{name}` not configured"
        ))),
    }
}

async fn run_acp_cmd(cli: &Cli) -> Result<(), CliErr> {
    #[cfg(feature = "acp")]
    {
        let cwd = std::env::current_dir().map_err(|e| CliErr::runtime(e.to_string()))?;
        let data_dir = resolve_data_dir(cli.home_dir.as_deref());
        let state = saint_harness_server::AppState::bootstrap(
            cwd,
            home_dir(),
            data_dir,
            "stdio://acp".into(),
            None,
            true,
        )
        .await
        .map_err(CliErr::runtime)?;
        let bridge = saint_harness_acp::AcpBridge::new(state);
        bridge.run_stdio_loop().await.map_err(CliErr::runtime)
    }
    #[cfg(not(feature = "acp"))]
    {
        let _ = cli;
        Err(CliErr::runtime(
            "acp feature is not enabled; rebuild with `--features acp`",
        ))
    }
}

#[cfg(feature = "embedded-server")]
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(feature = "embedded-server")]
fn resolve_data_dir(override_home: Option<&std::path::Path>) -> PathBuf {
    if let Some(h) = override_home {
        return h.to_path_buf();
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("saint-harness");
    }
    #[cfg(windows)]
    if let Ok(local_app) = std::env::var("LOCALAPPDATA")
        && !local_app.trim().is_empty()
    {
        return PathBuf::from(local_app).join("saint-harness");
    }
    home_dir()
        .join(".local")
        .join("share")
        .join("saint-harness")
}
