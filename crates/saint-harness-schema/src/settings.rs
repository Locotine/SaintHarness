use crate::{RouteDescriptor, RoutesConfig, SchemaError, embedded_routes};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Local user/project settings stored in `~/.saint-harness/settings.json`
/// (and optional workspace overrides `.saint-harness/settings.json` / `.saint-harness/settings.local.json`).
/// Never written to SQLite and never synced externally.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalSettings {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<String, RouteOverride>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderAuthPreset {
    pub id: &'static str,
    pub route_key: &'static str,
    pub env_key: &'static str,
    pub protocol: &'static str,
    pub default_base_url: &'static str,
    pub default_models_endpoint: &'static str,
    pub default_profile: &'static str,
}

pub const PROVIDER_AUTH_PRESETS: &[ProviderAuthPreset] = &[
    ProviderAuthPreset {
        id: "anthropic",
        route_key: "anthropic_direct",
        env_key: "ANTHROPIC_API_KEY",
        protocol: "anthropic_messages",
        default_base_url: "https://api.anthropic.com",
        default_models_endpoint: "/v1/models",
        default_profile: "claude-flagship",
    },
    ProviderAuthPreset {
        id: "gemini",
        route_key: "google_ai_studio",
        env_key: "GEMINI_API_KEY",
        protocol: "gemini_generate_content",
        default_base_url: "https://generativelanguage.googleapis.com",
        default_models_endpoint: "/v1beta/models",
        default_profile: "gemini-flagship",
    },
    ProviderAuthPreset {
        id: "dashscope",
        route_key: "qwen_dashscope",
        env_key: "DASHSCOPE_API_KEY",
        protocol: "openai_chat_compat",
        default_base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
        default_models_endpoint: "/models",
        default_profile: "qwen-3.8-27b",
    },
    ProviderAuthPreset {
        id: "local",
        route_key: "qwen_local_sglang",
        env_key: "SAINT_HARNESS_LOCAL_API_KEY",
        protocol: "openai_chat_compat",
        default_base_url: "http://127.0.0.1:30000/v1",
        default_models_endpoint: "/models",
        default_profile: "qwen-3.8-27b",
    },
    ProviderAuthPreset {
        id: "vllm",
        route_key: "qwen_local_vllm",
        env_key: "SAINT_HARNESS_LOCAL_API_KEY",
        protocol: "openai_chat_compat",
        default_base_url: "http://127.0.0.1:8000/v1",
        default_models_endpoint: "/models",
        default_profile: "qwen-3.8-27b",
    },
    ProviderAuthPreset {
        id: "openai",
        route_key: "openai_compat",
        env_key: "OPENAI_API_KEY",
        protocol: "openai_chat_compat",
        default_base_url: "https://api.openai.com/v1",
        default_models_endpoint: "/models",
        default_profile: "qwen-3.8-27b",
    },
];

pub fn find_provider_preset(name: &str) -> Option<&'static ProviderAuthPreset> {
    let norm = name.trim().to_ascii_lowercase();
    match norm.as_str() {
        "claude" | "anthropic" | "anthropic_direct" => Some(&PROVIDER_AUTH_PRESETS[0]),
        "google" | "gemini" | "google_ai_studio" => Some(&PROVIDER_AUTH_PRESETS[1]),
        "qwen" | "dashscope" | "qwen_dashscope" => Some(&PROVIDER_AUTH_PRESETS[2]),
        "local" | "sglang" | "qwen_local_sglang" => Some(&PROVIDER_AUTH_PRESETS[3]),
        "vllm" | "qwen_local_vllm" => Some(&PROVIDER_AUTH_PRESETS[4]),
        "openai" | "custom" | "openai_compat" => Some(&PROVIDER_AUTH_PRESETS[5]),
        _ => None,
    }
}

pub fn resolve_os_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Resolves the canonical user `settings.json` path:
/// 1. explicit `home_override` (`<home_override>/settings.json`)
/// 2. `SAINT_HARNESS_SETTINGS` env var (exact file path)
/// 3. `SAINT_HARNESS_HOME` env var (`<SAINT_HARNESS_HOME>/settings.json`)
/// 4. `~/.saint-harness/settings.json`
pub fn user_settings_path(home_override: Option<&Path>) -> PathBuf {
    if let Some(dir) = home_override {
        return dir.join("settings.json");
    }
    if let Ok(exact) = std::env::var("SAINT_HARNESS_SETTINGS")
        && !exact.trim().is_empty()
    {
        return PathBuf::from(exact);
    }
    if let Ok(home) = std::env::var("SAINT_HARNESS_HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join("settings.json");
    }
    resolve_os_home()
        .join(".saint-harness")
        .join("settings.json")
}

pub fn load_settings_file(path: &Path) -> LocalSettings {
    let Ok(raw) = fs::read_to_string(path) else {
        return LocalSettings::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn merge_settings(base: &mut LocalSettings, overlay: LocalSettings) {
    for (k, v) in overlay.env {
        base.env.insert(k, v);
    }
    if overlay.default_profile.is_some() {
        base.default_profile = overlay.default_profile;
    }
    for (rk, rov) in overlay.routes {
        let entry = base.routes.entry(rk).or_default();
        if rov.protocol.is_some() {
            entry.protocol = rov.protocol;
        }
        if rov.base_url.is_some() {
            entry.base_url = rov.base_url;
        }
        if rov.models_endpoint.is_some() {
            entry.models_endpoint = rov.models_endpoint;
        }
        if rov.api_key_env.is_some() {
            entry.api_key_env = rov.api_key_env;
        }
    }
}

/// Loads effective settings merged in ascending priority:
/// user `~/.saint-harness/settings.json` < `<cwd>/.saint-harness/settings.json` < `<cwd>/.saint-harness/settings.local.json`.
pub fn load_effective_settings(cwd: Option<&Path>, home_override: Option<&Path>) -> LocalSettings {
    let mut merged = load_settings_file(&user_settings_path(home_override));
    let effective_cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    if let Some(dir) = effective_cwd {
        let proj = dir.join(".saint-harness").join("settings.json");
        if proj.exists() {
            merge_settings(&mut merged, load_settings_file(&proj));
        }
        let proj_local = dir.join(".saint-harness").join("settings.local.json");
        if proj_local.exists() {
            merge_settings(&mut merged, load_settings_file(&proj_local));
        }
    }
    merged
}

/// Resolves an API key or config variable with clear provenance:
/// 1. Process environment variable
/// 2. `<cwd>/.saint-harness/settings.local.json` -> `env`
/// 3. `<cwd>/.saint-harness/settings.json` -> `env`
/// 4. `~/.saint-harness/settings.json` -> `env`
pub fn resolve_env_value(
    key: &str,
    cwd: Option<&Path>,
    home_override: Option<&Path>,
) -> Option<(String, &'static str)> {
    if let Ok(val) = std::env::var(key)
        && !val.trim().is_empty()
    {
        return Some((val, "env"));
    }

    let effective_cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    if let Some(dir) = effective_cwd {
        let proj_local = dir.join(".saint-harness").join("settings.local.json");
        if let Some(v) = load_settings_file(&proj_local)
            .env
            .get(key)
            .filter(|s| !s.trim().is_empty())
        {
            return Some((v.clone(), "settings.local.json"));
        }
        let proj = dir.join(".saint-harness").join("settings.json");
        if let Some(v) = load_settings_file(&proj)
            .env
            .get(key)
            .filter(|s| !s.trim().is_empty())
        {
            return Some((v.clone(), "project:settings.json"));
        }
    }

    let user_path = user_settings_path(home_override);
    if let Some(v) = load_settings_file(&user_path)
        .env
        .get(key)
        .filter(|s| !s.trim().is_empty())
    {
        return Some((v.clone(), "settings.json"));
    }

    None
}

/// Convenience helper used by HTTP wire protocols and catalog discovery.
pub fn resolve_api_key(key: &str) -> Option<String> {
    resolve_env_value(key, None, None).map(|(v, _)| v)
}

/// Atomically writes `settings.json` with restrictive permissions (`0700` dir, `0600` file on Unix).
pub fn save_user_settings(
    settings: &LocalSettings,
    home_override: Option<&Path>,
) -> Result<PathBuf, String> {
    let target = user_settings_path(home_override);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let mut json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    json.push('\n');

    let tmp_path = target.with_extension("json.tmp");
    fs::write(&tmp_path, json.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("failed to chmod 0600 {}: {e}", tmp_path.display()))?;
    }

    fs::rename(&tmp_path, &target)
        .map_err(|e| format!("failed to move {} into place: {e}", target.display()))?;
    Ok(target)
}

/// Applies local `settings.json` route overrides and `*_BASE_URL` env vars onto `RoutesConfig`.
pub fn apply_route_overrides(
    routes: &mut RoutesConfig,
    settings: &LocalSettings,
    cwd: Option<&Path>,
    home_override: Option<&Path>,
) {
    for (route_key, ov) in &settings.routes {
        let entry = routes
            .routes
            .entry(route_key.clone())
            .or_insert_with(|| RouteDescriptor {
                protocol: ov
                    .protocol
                    .clone()
                    .unwrap_or_else(|| "openai_chat_compat".to_string()),
                base_url: ov.base_url.clone().unwrap_or_default(),
                models_endpoint: ov
                    .models_endpoint
                    .clone()
                    .unwrap_or_else(|| "/models".to_string()),
                api_key_env: ov
                    .api_key_env
                    .clone()
                    .unwrap_or_else(|| "OPENAI_API_KEY".to_string()),
                prefix_cache_engine: None,
                context_editing_strategy: None,
                beta_headers: Vec::new(),
                clear_tool_uses_type: None,
            });
        if let Some(ref proto) = ov.protocol {
            entry.protocol = proto.clone();
        }
        if let Some(ref base) = ov.base_url {
            entry.base_url = base.clone();
        }
        if let Some(ref ep) = ov.models_endpoint {
            entry.models_endpoint = ep.clone();
        }
        if let Some(ref env_k) = ov.api_key_env {
            entry.api_key_env = env_k.clone();
        }
    }

    // Standard base URL env overrides (from process env or settings.json `env`).
    let url_overrides = [
        ("ANTHROPIC_BASE_URL", "anthropic_direct"),
        ("GEMINI_BASE_URL", "google_ai_studio"),
        ("DASHSCOPE_BASE_URL", "qwen_dashscope"),
        ("SAINT_HARNESS_LOCAL_BASE_URL", "qwen_local_sglang"),
    ];
    for (env_name, route_key) in url_overrides {
        if let Some((val, _)) = resolve_env_value(env_name, cwd, home_override)
            && let Some(route) = routes.routes.get_mut(route_key)
        {
            route.base_url = val;
        }
    }
}

pub fn load_routes_with_overrides(
    cwd: Option<&Path>,
    home_override: Option<&Path>,
) -> Result<RoutesConfig, SchemaError> {
    let mut routes = embedded_routes()?;
    let settings = load_effective_settings(cwd, home_override);
    apply_route_overrides(&mut routes, &settings, cwd, home_override);
    Ok(routes)
}

/// Masks a secret for display (`sk-ant-...9f2a`) without revealing the body.
pub fn mask_secret_preview(raw: &str) -> String {
    let trimmed = raw.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 10 {
        return "***".to_string();
    }
    let head: String = chars.iter().take(6).collect();
    let tail: String = chars.iter().skip(chars.len() - 4).collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn settings_json_save_load_precedence_and_route_override() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();

        let mut user = LocalSettings::default();
        user.env.insert(
            "TEST_SH_KEY_ONLY_FILE".into(),
            "sk-ant-1234567890abcd".into(),
        );
        user.routes.insert(
            "anthropic_direct".into(),
            RouteOverride {
                base_url: Some("https://proxy.internal/anthropic".into()),
                ..RouteOverride::default()
            },
        );
        let saved_path = save_user_settings(&user, Some(&home)).unwrap();
        assert!(saved_path.exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&saved_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "settings.json must be chmod 0600");
        }

        let (val, src) =
            resolve_env_value("TEST_SH_KEY_ONLY_FILE", Some(&ws), Some(&home)).unwrap();
        assert_eq!(val, "sk-ant-1234567890abcd");
        assert_eq!(src, "settings.json");
        assert_eq!(mask_secret_preview(&val), "sk-ant...abcd");

        // Workspace settings.local.json overrides user settings.json
        let ws_cfg_dir = ws.join(".saint-harness");
        fs::create_dir_all(&ws_cfg_dir).unwrap();
        fs::write(
            ws_cfg_dir.join("settings.local.json"),
            r#"{"env":{"TEST_SH_KEY_ONLY_FILE":"sk-local-override-9999"}}"#,
        )
        .unwrap();
        let (val2, src2) =
            resolve_env_value("TEST_SH_KEY_ONLY_FILE", Some(&ws), Some(&home)).unwrap();
        assert_eq!(val2, "sk-local-override-9999");
        assert_eq!(src2, "settings.local.json");

        // Route overrides apply cleanly
        let routes = load_routes_with_overrides(Some(&ws), Some(&home)).unwrap();
        assert_eq!(
            routes.routes["anthropic_direct"].base_url,
            "https://proxy.internal/anthropic"
        );
    }
}
