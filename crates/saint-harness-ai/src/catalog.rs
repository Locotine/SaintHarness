use chrono::Utc;
use regex::Regex;
use saint_harness_schema::{ModelProfile, ResolvedModel, RouteDescriptor};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const CACHE_TTL_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModelEntry {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    #[serde(default)]
    pub supports_adaptive_thinking: bool,
    #[serde(default)]
    pub supports_vision: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskCatalogCache {
    etag: Option<String>,
    fetched_at_unix: i64,
    models: Vec<CatalogModelEntry>,
}

/// Resolves model IDs dynamically without hardcoding provider version numbers in Rust.
/// Priority: (1) CLI `--model` override -> (2) env override -> (3) `/models` catalog + ETag disk cache -> (4) embedded selector fallback.
pub struct ModelResolver {
    cache_dir: Option<PathBuf>,
    http: reqwest::Client,
}

impl Default for ModelResolver {
    fn default() -> Self {
        Self::new(None)
    }
}

impl ModelResolver {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { cache_dir, http }
    }

    pub async fn resolve(
        &self,
        profile: &ModelProfile,
        route: &RouteDescriptor,
        cli_model_override: Option<&str>,
        force_refresh: bool,
    ) -> ResolvedModel {
        // 1. Explicit CLI --model override
        if let Some(model_id) = cli_model_override.filter(|s| !s.trim().is_empty()) {
            return build_resolved(
                profile,
                model_id.trim().to_string(),
                None,
                "cli_override".to_string(),
            );
        }

        // 2. Environment or settings.json override from profile.model.env_override
        if let Some(env_model) = saint_harness_schema::resolve_api_key(&profile.model.env_override)
        {
            return build_resolved(
                profile,
                env_model.trim().to_string(),
                None,
                format!("env:{}", profile.model.env_override),
            );
        }

        // 3. Exact selector does not require discovery unless metadata enrichment is cached
        if let Some(exact_id) = profile.model.selector.strip_prefix("exact:") {
            return build_resolved(
                profile,
                exact_id.to_string(),
                None,
                "selector:exact".to_string(),
            );
        }

        // 4. Live/cached provider `/models` discovery
        if let Ok(entries) = self.fetch_catalog(profile, route, force_refresh).await
            && let Some(chosen) =
                select_from_catalog(&profile.model.provider, &profile.model.selector, &entries)
        {
            return build_resolved(
                profile,
                chosen.id.clone(),
                Some(&chosen),
                "catalog:live".to_string(),
            );
        }

        // 5. Embedded fallback metadata (selector-based alias, zero hardcoded provider release numbers)
        let fallback_id = embedded_fallback_id(&profile.model.provider, &profile.model.selector);
        build_resolved(
            profile,
            fallback_id,
            None,
            "catalog:embedded_fallback".to_string(),
        )
    }

    pub fn resolve_from_entries(
        &self,
        profile: &ModelProfile,
        cli_model_override: Option<&str>,
        entries: &[CatalogModelEntry],
    ) -> ResolvedModel {
        if let Some(model_id) = cli_model_override.filter(|s| !s.trim().is_empty()) {
            return build_resolved(
                profile,
                model_id.trim().to_string(),
                None,
                "cli_override".to_string(),
            );
        }
        if let Some(exact_id) = profile.model.selector.strip_prefix("exact:") {
            return build_resolved(
                profile,
                exact_id.to_string(),
                None,
                "selector:exact".to_string(),
            );
        }
        if let Some(chosen) =
            select_from_catalog(&profile.model.provider, &profile.model.selector, entries)
        {
            return build_resolved(
                profile,
                chosen.id.clone(),
                Some(&chosen),
                "catalog:entries".to_string(),
            );
        }
        build_resolved(
            profile,
            embedded_fallback_id(&profile.model.provider, &profile.model.selector),
            None,
            "catalog:embedded_fallback".to_string(),
        )
    }

    pub async fn fetch_catalog(
        &self,
        profile: &ModelProfile,
        route: &RouteDescriptor,
        force_refresh: bool,
    ) -> Result<Vec<CatalogModelEntry>, String> {
        let cache_file = self.cache_path(&profile.model.route);
        let cached = cache_file
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<DiskCatalogCache>(&s).ok());

        let now = Utc::now().timestamp();
        if !force_refresh
            && let Some(ref c) = cached
            && now.saturating_sub(c.fetched_at_unix) < CACHE_TTL_SECS
            && !c.models.is_empty()
        {
            return Ok(c.models.clone());
        }

        let api_key = saint_harness_schema::resolve_api_key(&route.api_key_env);
        let url = format!(
            "{}{}",
            route.base_url.trim_end_matches('/'),
            route.models_endpoint
        );

        let mut req = self.http.get(&url);
        if let Some(ref c) = cached
            && let Some(ref etag) = c.etag
        {
            req = req.header("If-None-Match", etag);
        }
        if let Some(key) = api_key {
            match route.protocol.as_str() {
                "anthropic_messages" => {
                    req = req
                        .header("x-api-key", key)
                        .header("anthropic-version", "2023-06-01");
                }
                "gemini_generate_content" => {
                    req = req.header("x-goog-api-key", key);
                }
                _ => {
                    req = req.bearer_auth(key);
                }
            }
        }

        let resp = req.send().await.map_err(|e| e.to_string())?;
        if resp.status().as_u16() == 304
            && let Some(mut c) = cached
        {
            c.fetched_at_unix = now;
            self.write_cache(cache_file.as_deref(), &c);
            return Ok(c.models);
        }

        if !resp.status().is_success() {
            return Err(format!("models endpoint returned {}", resp.status()));
        }

        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let models = parse_provider_models_payload(&profile.model.provider, &body);
        let disk = DiskCatalogCache {
            etag,
            fetched_at_unix: now,
            models: models.clone(),
        };
        self.write_cache(cache_file.as_deref(), &disk);
        Ok(models)
    }

    fn cache_path(&self, route_name: &str) -> Option<PathBuf> {
        self.cache_dir
            .as_ref()
            .map(|dir| dir.join(format!("catalog-{route_name}.json")))
    }

    fn write_cache(&self, path: Option<&Path>, cache: &DiskCatalogCache) {
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(path, json);
            }
        }
    }
}

pub fn parse_provider_models_payload(
    provider: &str,
    payload: &serde_json::Value,
) -> Vec<CatalogModelEntry> {
    let mut out = Vec::new();
    let list = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(|v| v.as_array());
    let Some(items) = list else {
        return out;
    };
    for item in items {
        let raw_id = item
            .get("id")
            .or_else(|| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if raw_id.is_empty() {
            continue;
        }
        let id = raw_id.strip_prefix("models/").unwrap_or(raw_id).to_string();
        let display_name = item
            .get("display_name")
            .or_else(|| item.get("displayName"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string);
        let created = item.get("created").and_then(|v| v.as_i64());
        let context_window = item
            .get("context_window")
            .or_else(|| item.get("inputTokenLimit"))
            .and_then(|v| v.as_u64());
        let max_output_tokens = item
            .get("max_output_tokens")
            .or_else(|| item.get("outputTokenLimit"))
            .and_then(|v| v.as_u64());
        let supports_adaptive_thinking = provider == "anthropic" || provider == "google";
        let supports_vision = true;
        out.push(CatalogModelEntry {
            id,
            display_name,
            created,
            context_window,
            max_output_tokens,
            supports_adaptive_thinking,
            supports_vision,
        });
    }
    out
}

pub fn select_from_catalog(
    provider: &str,
    selector: &str,
    entries: &[CatalogModelEntry],
) -> Option<CatalogModelEntry> {
    if let Some(exact) = selector.strip_prefix("exact:") {
        return entries.iter().find(|e| e.id == exact).cloned();
    }

    let family_prefix = selector.strip_prefix("latest_family:");
    let want_fast = selector == "latest_fast";

    let mut candidates: Vec<&CatalogModelEntry> = entries
        .iter()
        .filter(|e| {
            let lower = e.id.to_ascii_lowercase();
            if lower.contains("embed")
                || lower.contains("tts")
                || lower.contains("whisper")
                || lower.contains("imagen")
                || lower.contains("audio")
                || lower.contains("moderation")
            {
                return false;
            }
            if let Some(prefix) = family_prefix {
                return matches_family(&lower, prefix);
            }
            match provider {
                "anthropic" => lower.contains("claude"),
                "google" => lower.contains("gemini"),
                "qwen" => lower.contains("qwen"),
                _ => true,
            }
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    candidates.sort_by(|a, b| {
        let sa = rank_model(provider, want_fast, a);
        let sb = rank_model(provider, want_fast, b);
        sb.cmp(&sa)
    });

    candidates.first().copied().cloned()
}

fn matches_family(id_lower: &str, prefix: &str) -> bool {
    let norm_id = id_lower.replace(['-', '_', '.'], "");
    let norm_prefix = prefix.to_ascii_lowercase().replace(['-', '_', '.'], "");
    if norm_id.contains(&norm_prefix) {
        return true;
    }
    prefix
        .split('-')
        .all(|tok| !tok.is_empty() && id_lower.contains(&tok.to_ascii_lowercase()))
}

fn rank_model(
    provider: &str,
    want_fast: bool,
    entry: &CatalogModelEntry,
) -> (u8, Vec<u64>, i64, String) {
    let lower = entry.id.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split(['-', '_', '.']).collect();
    let has_tok = |needle: &str| tokens.contains(&needle);
    let tier_score = if want_fast {
        if has_tok("flash") || has_tok("haiku") || has_tok("mini") {
            3
        } else if has_tok("sonnet") || has_tok("pro") {
            2
        } else {
            1
        }
    } else {
        match provider {
            "anthropic" => {
                if has_tok("opus") {
                    4
                } else if has_tok("sonnet") {
                    3
                } else if has_tok("fable") {
                    2
                } else {
                    1
                }
            }
            "google" => {
                if has_tok("pro") && !has_tok("lite") {
                    4
                } else if has_tok("flash") && !has_tok("lite") {
                    3
                } else {
                    1
                }
            }
            _ => {
                if has_tok("max") || has_tok("plus") || has_tok("coder") {
                    3
                } else {
                    2
                }
            }
        }
    };
    let nums = extract_numeric_version_tuple(&lower);
    let created = entry.created.unwrap_or(0);
    (tier_score, nums, created, entry.id.clone())
}

fn extract_numeric_version_tuple(s: &str) -> Vec<u64> {
    static NUM_RE: OnceLock<Regex> = OnceLock::new();
    let re = NUM_RE.get_or_init(|| Regex::new(r"\d+").unwrap_or_else(|_| unreachable!()));
    re.find_iter(s)
        .filter_map(|m| m.as_str().parse::<u64>().ok())
        .filter(|n| *n < 20000000) // Ignore 8-digit date suffixes when comparing major/minor versions
        .take(4)
        .collect()
}

fn embedded_fallback_id(provider: &str, selector: &str) -> String {
    if let Some(exact) = selector.strip_prefix("exact:") {
        return exact.to_string();
    }
    match (provider, selector) {
        ("anthropic", "latest_fast") => "claude-fast-latest".to_string(),
        ("anthropic", _) => "claude-flagship-latest".to_string(),
        ("google", "latest_fast") => "gemini-flash-latest".to_string(),
        ("google", _) => "gemini-pro-latest".to_string(),
        ("qwen", s) if s.contains("qwen4") => "qwen4-latest".to_string(),
        _ => "qwen3.8-27b".to_string(),
    }
}

fn build_resolved(
    profile: &ModelProfile,
    model_id: String,
    catalog_entry: Option<&CatalogModelEntry>,
    source: String,
) -> ResolvedModel {
    let default_native = match profile.model.provider.as_str() {
        "google" => 1_000_000,
        "anthropic" => 200_000,
        _ => 262_144,
    };
    let catalog_native = catalog_entry
        .and_then(|e| e.context_window)
        .unwrap_or(default_native);
    let native_window = profile.context.native_window.resolve(catalog_native);
    let computed_effective = native_window.saturating_sub((native_window / 7).max(16_000));
    let effective_window = profile
        .context
        .effective_window
        .resolve(computed_effective.max(32_000));
    let default_max_out = catalog_entry
        .and_then(|e| e.max_output_tokens)
        .unwrap_or(32_000);
    let max_output_tokens = profile.context.max_output_tokens.resolve(default_max_out);
    let display_name = catalog_entry
        .and_then(|e| e.display_name.clone())
        .unwrap_or_else(|| model_id.clone());

    ResolvedModel {
        provider: profile.model.provider.clone(),
        model_id,
        display_name,
        native_window,
        effective_window,
        max_output_tokens,
        supports_adaptive_thinking: catalog_entry
            .map(|e| e.supports_adaptive_thinking)
            .unwrap_or(profile.model.provider == "anthropic"),
        supports_vision: catalog_entry.map(|e| e.supports_vision).unwrap_or(true),
        resolution_source: source,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use saint_harness_schema::embedded_profile;

    #[test]
    fn resolver_picks_highest_future_flagship_dynamically() {
        let profile = embedded_profile("claude-flagship").unwrap();
        let resolver = ModelResolver::default();
        let future_catalog = vec![
            CatalogModelEntry {
                id: "claude-haiku-8-1".to_string(),
                display_name: Some("Claude Haiku 8.1".to_string()),
                created: Some(1800000000),
                context_window: Some(400_000),
                max_output_tokens: Some(64_000),
                supports_adaptive_thinking: true,
                supports_vision: true,
            },
            CatalogModelEntry {
                id: "claude-opus-9-2".to_string(),
                display_name: Some("Claude Opus 9.2".to_string()),
                created: Some(1900000000),
                context_window: Some(500_000),
                max_output_tokens: Some(64_000),
                supports_adaptive_thinking: true,
                supports_vision: true,
            },
            CatalogModelEntry {
                id: "claude-opus-8-0".to_string(),
                display_name: Some("Claude Opus 8.0".to_string()),
                created: Some(1750000000),
                context_window: Some(200_000),
                max_output_tokens: Some(32_000),
                supports_adaptive_thinking: true,
                supports_vision: true,
            },
        ];
        let resolved = resolver.resolve_from_entries(&profile, None, &future_catalog);
        assert_eq!(resolved.model_id, "claude-opus-9-2");
        assert_eq!(resolved.native_window, 500_000);
        assert_eq!(resolved.max_output_tokens, 64_000);
    }

    #[test]
    fn resolver_handles_future_gemini_flagship_and_fast() {
        let mut profile = embedded_profile("gemini-flagship").unwrap();
        let resolver = ModelResolver::default();
        let future_catalog = vec![
            CatalogModelEntry {
                id: "gemini-7-flash".to_string(),
                display_name: None,
                created: Some(1900000000),
                context_window: Some(2_000_000),
                max_output_tokens: Some(65_536),
                supports_adaptive_thinking: true,
                supports_vision: true,
            },
            CatalogModelEntry {
                id: "gemini-7-pro".to_string(),
                display_name: None,
                created: Some(1900000000),
                context_window: Some(2_000_000),
                max_output_tokens: Some(65_536),
                supports_adaptive_thinking: true,
                supports_vision: true,
            },
        ];
        let flagship = resolver.resolve_from_entries(&profile, None, &future_catalog);
        assert_eq!(flagship.model_id, "gemini-7-pro");

        profile.model.selector = "latest_fast".to_string();
        let fast = resolver.resolve_from_entries(&profile, None, &future_catalog);
        assert_eq!(fast.model_id, "gemini-7-flash");
    }

    #[test]
    fn cli_override_beats_catalog_and_selector() {
        let profile = embedded_profile("qwen-3.8-27b").unwrap();
        let resolver = ModelResolver::default();
        let resolved = resolver.resolve_from_entries(&profile, Some("custom-local-model"), &[]);
        assert_eq!(resolved.model_id, "custom-local-model");
        assert_eq!(resolved.resolution_source, "cli_override");
    }
}
