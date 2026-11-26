use clap::Subcommand;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use saint_harness_schema::{
    PROVIDER_AUTH_PRESETS, RouteOverride, find_provider_preset, load_effective_settings,
    load_routes_with_overrides, load_settings_file, mask_secret_preview, resolve_env_value,
    save_user_settings, user_settings_path,
};
use serde_json::json;
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Subcommand)]
pub enum AuthAction {
    /// Save a provider API key and optional custom base_url into local settings.json (chmod 0600)
    Login {
        /// Provider preset: anthropic | gemini | dashscope | local | vllm | openai
        #[arg(long)]
        provider: Option<String>,
        /// Custom API base URL override (e.g. OpenRouter, LiteLLM, corporate proxy, SGLang)
        #[arg(long)]
        base_url: Option<String>,
        /// Custom models discovery path (e.g. /v1/models or /models)
        #[arg(long)]
        models_endpoint: Option<String>,
        /// Custom env key name inside settings.json `env` object
        #[arg(long)]
        env_key: Option<String>,
        /// Read API key from stdin (for non-interactive scripts without leaking to shell history)
        #[arg(long)]
        api_key_stdin: bool,
        /// Skip live GET /models probe after saving
        #[arg(long)]
        no_verify: bool,
        /// Set this provider's profile as `defaultProfile` in settings.json
        #[arg(long)]
        set_default: bool,
    },
    /// Show configured provider keys (masked), provenance, and active routes
    Status,
    /// Remove a saved provider key from local settings.json
    Logout {
        /// Provider preset (anthropic | gemini | dashscope | local | openai) or exact env key name
        provider: String,
    },
    /// Print the local settings.json file path
    Path,
}

pub async fn run_auth_cmd(
    action: AuthAction,
    home_override: Option<&Path>,
    json_out: bool,
) -> Result<(), String> {
    match action {
        AuthAction::Path => {
            let path = user_settings_path(home_override);
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "path": path.display().to_string(),
                        "exists": path.exists()
                    }))
                    .unwrap_or_else(|_| "{}".into())
                );
            } else {
                println!("{}", path.display());
            }
            Ok(())
        }
        AuthAction::Status => run_auth_status(home_override, json_out),
        AuthAction::Logout { provider } => run_auth_logout(&provider, home_override, json_out),
        AuthAction::Login {
            provider,
            base_url,
            models_endpoint,
            env_key,
            api_key_stdin,
            no_verify,
            set_default,
        } => {
            run_auth_login(
                provider,
                base_url,
                models_endpoint,
                env_key,
                api_key_stdin,
                no_verify,
                set_default,
                home_override,
                json_out,
            )
            .await
        }
    }
}

fn run_auth_status(home_override: Option<&Path>, json_out: bool) -> Result<(), String> {
    let cwd = std::env::current_dir().ok();
    let settings_path = user_settings_path(home_override);
    let routes =
        load_routes_with_overrides(cwd.as_deref(), home_override).map_err(|e| e.to_string())?;
    let effective = load_effective_settings(cwd.as_deref(), home_override);

    let mut rows = Vec::new();
    for (route_key, route) in &routes.routes {
        let resolved = resolve_env_value(&route.api_key_env, cwd.as_deref(), home_override);
        let (configured, source, preview) = match resolved {
            Some((val, src)) => (true, src, Some(mask_secret_preview(&val))),
            None => (false, "unset", None),
        };
        rows.push(json!({
            "route": route_key,
            "protocol": route.protocol,
            "base_url": route.base_url,
            "env_key": route.api_key_env,
            "configured": configured,
            "source": source,
            "preview": preview,
        }));
    }

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "settings_path": settings_path.display().to_string(),
                "settings_exists": settings_path.exists(),
                "default_profile": effective.default_profile,
                "routes": rows,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!("Settings file: {}", settings_path.display());
        if let Some(ref dp) = effective.default_profile {
            println!("Default profile: {dp}");
        }
        println!();
        for r in &rows {
            let route = r["route"].as_str().unwrap_or("");
            let env_k = r["env_key"].as_str().unwrap_or("");
            let src = r["source"].as_str().unwrap_or("unset");
            let base = r["base_url"].as_str().unwrap_or("");
            let status = if r["configured"].as_bool().unwrap_or(false) {
                let prev = r["preview"].as_str().unwrap_or("***");
                format!("configured ({src}: {prev})")
            } else {
                "not set".to_string()
            };
            println!("{route:<20} {env_k:<28} {status:<36} {base}");
        }
    }
    Ok(())
}

fn run_auth_logout(
    provider: &str,
    home_override: Option<&Path>,
    json_out: bool,
) -> Result<(), String> {
    let settings_path = user_settings_path(home_override);
    let mut settings = load_settings_file(&settings_path);
    let env_key = find_provider_preset(provider)
        .map(|p| p.env_key.to_string())
        .unwrap_or_else(|| provider.trim().to_string());
    let removed = settings.env.remove(&env_key).is_some();
    let saved = save_user_settings(&settings, home_override)?;

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "removed": removed,
                "env_key": env_key,
                "settings_path": saved.display().to_string(),
            }))
            .unwrap_or_else(|_| "{}".into())
        );
    } else if removed {
        println!("Removed `{env_key}` from {}", saved.display());
    } else {
        println!("`{env_key}` was not set in {}", saved.display());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_auth_login(
    provider_arg: Option<String>,
    base_url: Option<String>,
    models_endpoint: Option<String>,
    env_key_arg: Option<String>,
    api_key_stdin: bool,
    no_verify: bool,
    set_default: bool,
    home_override: Option<&Path>,
    json_out: bool,
) -> Result<(), String> {
    let provider_choice = match provider_arg {
        Some(p) => p,
        None => {
            if !io::stdin().is_terminal() {
                return Err(
                    "non-interactive login requires `--provider <anthropic|gemini|dashscope|local|vllm|openai>`"
                        .to_string(),
                );
            }
            prompt_provider_choice()?
        }
    };

    let preset = find_provider_preset(&provider_choice).ok_or_else(|| {
        format!(
            "unknown provider `{provider_choice}` (choose: anthropic, gemini, dashscope, local, vllm, openai)"
        )
    })?;

    let env_key = env_key_arg.unwrap_or_else(|| preset.env_key.to_string());
    let api_key = if api_key_stdin || !io::stdin().is_terminal() {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read API key from stdin: {e}"))?;
        buf.trim().to_string()
    } else {
        read_masked_secret(&format!("Paste API key for {} ({env_key}): ", preset.id))?
    };

    if api_key.is_empty() && preset.id != "local" && preset.id != "vllm" {
        return Err("API key cannot be empty".to_string());
    }

    let settings_path = user_settings_path(home_override);
    let mut settings = load_settings_file(&settings_path);
    if !api_key.is_empty() {
        settings.env.insert(env_key.clone(), api_key.clone());
    }
    if set_default {
        settings.default_profile = Some(preset.default_profile.to_string());
    }

    let effective_base_url = base_url
        .as_deref()
        .unwrap_or(preset.default_base_url)
        .to_string();
    let effective_models_ep = models_endpoint
        .as_deref()
        .unwrap_or(preset.default_models_endpoint)
        .to_string();

    if base_url.is_some() || models_endpoint.is_some() || preset.id == "openai" {
        settings.routes.insert(
            preset.route_key.to_string(),
            RouteOverride {
                protocol: Some(preset.protocol.to_string()),
                base_url: Some(effective_base_url.clone()),
                models_endpoint: Some(effective_models_ep.clone()),
                api_key_env: Some(env_key.clone()),
            },
        );
    }

    let saved = save_user_settings(&settings, home_override)?;

    let verify_result = if no_verify {
        None
    } else {
        Some(
            verify_provider_endpoint(
                preset.protocol,
                &effective_base_url,
                &effective_models_ep,
                &api_key,
            )
            .await,
        )
    };

    let preview = if api_key.is_empty() {
        "(none)".to_string()
    } else {
        mask_secret_preview(&api_key)
    };

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "saved": true,
                "provider": preset.id,
                "route": preset.route_key,
                "env_key": env_key,
                "preview": preview,
                "base_url": effective_base_url,
                "settings_path": saved.display().to_string(),
                "verified": verify_result.as_ref().map(|r| r.is_ok()),
                "verify_detail": verify_result.as_ref().map(|r| match r {
                    Ok(msg) => msg.clone(),
                    Err(err) => err.clone(),
                }),
            }))
            .unwrap_or_else(|_| "{}".into())
        );
    } else {
        println!("Saved `{env_key}` ({preview}) to {}", saved.display());
        if let Some(res) = verify_result {
            match res {
                Ok(msg) => println!("Verified endpoint: {msg}"),
                Err(warn) => eprintln!("Warning: saved locally, but live check failed: {warn}"),
            }
        }
    }
    Ok(())
}

fn prompt_provider_choice() -> Result<String, String> {
    println!("Select provider to store in local settings.json:");
    for (idx, p) in PROVIDER_AUTH_PRESETS.iter().enumerate() {
        println!(
            "  ({}) {:<10} {}  ({})",
            idx + 1,
            p.id,
            p.env_key,
            p.default_base_url
        );
    }
    print!("Choice [1-{}]: ", PROVIDER_AUTH_PRESETS.len());
    let _ = io::stdout().flush();
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("failed to read choice: {e}"))?;
    let trimmed = line.trim();
    if let Ok(num) = trimmed.parse::<usize>()
        && (1..=PROVIDER_AUTH_PRESETS.len()).contains(&num)
    {
        return Ok(PROVIDER_AUTH_PRESETS[num - 1].id.to_string());
    }
    Ok(trimmed.to_string())
}

struct RawModeGuard;
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn read_masked_secret(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    let _ = io::stdout().flush();
    enable_raw_mode().map_err(|e| format!("failed to enter raw mode: {e}"))?;
    let _guard = RawModeGuard;

    let mut secret = String::new();
    loop {
        match event::read().map_err(|e| format!("input error: {e}"))? {
            Event::Key(key) => match (key.code, key.modifiers) {
                (KeyCode::Enter, _) => {
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    break;
                }
                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                    print!("\r\n");
                    let _ = io::stdout().flush();
                    return Err("cancelled".to_string());
                }
                (KeyCode::Backspace, _) => {
                    if secret.pop().is_some() {
                        print!("\x08 \x08");
                        let _ = io::stdout().flush();
                    }
                }
                (KeyCode::Char(c), _) => {
                    secret.push(c);
                    print!("*");
                    let _ = io::stdout().flush();
                }
                _ => {}
            },
            Event::Paste(pasted) => {
                let cleaned = pasted.trim();
                for ch in cleaned.chars() {
                    secret.push(ch);
                    print!("*");
                }
                let _ = io::stdout().flush();
            }
            _ => {}
        }
    }
    Ok(secret.trim().to_string())
}

async fn verify_provider_endpoint(
    protocol: &str,
    base_url: &str,
    models_endpoint: &str,
    api_key: &str,
) -> Result<String, String> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), models_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    match protocol {
        "anthropic_messages" => {
            req = req.header("anthropic-version", "2023-06-01");
            if !api_key.is_empty() {
                req = req.header("x-api-key", api_key);
            }
        }
        "gemini_generate_content" => {
            if !api_key.is_empty() {
                req = req.header("x-goog-api-key", api_key);
            }
        }
        _ => {
            if !api_key.is_empty() {
                req = req.bearer_auth(api_key);
            }
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("GET {url} failed: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        Ok(format!("HTTP {} from {url}", status.as_u16()))
    } else {
        let body = resp.text().await.unwrap_or_default();
        let excerpt: String = body.chars().take(160).collect();
        Err(format!("HTTP {} from {url}: {excerpt}", status.as_u16()))
    }
}
