use crate::SchemaError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AutoOrU64 {
    Fixed(u64),
    Auto(String),
}

impl AutoOrU64 {
    pub fn resolve(&self, catalog_value: u64) -> u64 {
        match self {
            AutoOrU64::Fixed(v) => *v,
            AutoOrU64::Auto(_) => catalog_value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelectorSpec {
    pub provider: String,
    pub selector: String,
    pub env_override: String,
    pub route: String,
    pub protocol: String,
    pub prompt_tune_file: String,
    pub prompt_tune_mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSpec {
    pub native_window: AutoOrU64,
    pub effective_window: AutoOrU64,
    #[serde(default)]
    pub soft_shake_tokens: Option<u64>,
    #[serde(default)]
    pub soft_shake_ratio: Option<f64>,
    pub keep_recent_tokens: u64,
    pub max_output_tokens: AutoOrU64,
}

impl ContextSpec {
    pub fn soft_shake_threshold(&self, effective_window: u64) -> u64 {
        if let Some(tokens) = self.soft_shake_tokens {
            tokens
        } else if let Some(ratio) = self.soft_shake_ratio {
            ((effective_window as f64) * ratio).round() as u64
        } else {
            ((effective_window as f64) * 0.60).round() as u64
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningSpec {
    pub mode: String,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub prefix_mismatch_behavior: Option<String>,
    #[serde(default)]
    pub preserve_thinking: bool,
    #[serde(default)]
    pub thinking_entrapment_guard: bool,
    #[serde(default)]
    pub smol_disables_thinking: bool,
    #[serde(default)]
    pub preserve_thought_signature: bool,
    #[serde(default)]
    pub unsigned_tool_sentinel: Option<String>,
}

fn default_max_direct_tools() -> usize {
    12
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsSpec {
    pub edit_mode: String,
    pub diff_engine: String,
    #[serde(default)]
    pub ast_edit: bool,
    pub read_outline_threshold_lines: usize,
    #[serde(default)]
    pub delta_reread: bool,
    pub repomap_budget_tokens: usize,
    pub observation_masking_window: usize,
    #[serde(default)]
    pub fast_lint: Vec<String>,
    pub git_engine: String,
    pub ts_plugin_strip: String,
    pub tantivy_threshold_files: usize,
    #[serde(default = "default_max_direct_tools")]
    pub max_direct_tools: usize,
    #[serde(default)]
    pub wire_xml_fallback: bool,
    #[serde(default)]
    pub json_repair: bool,
    #[serde(default)]
    pub defer_loading: bool,
    #[serde(default)]
    pub clear_tool_uses_server: bool,
    #[serde(default)]
    pub strict_schema_sanitizer: bool,
    #[serde(default)]
    pub separate_user_turn_after_fn_resp: bool,
    #[serde(default)]
    pub safety_settings: Option<String>,
    pub policy_guard: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSpec {
    pub policy: String,
    pub ttl_seconds: u64,
    #[serde(default)]
    pub strict_warm_prune_guard: bool,
    #[serde(default)]
    pub allow_snapcompact: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingSpec {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub presence_penalty: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsSpec {
    pub doom_loop_repeat: usize,
    pub max_concurrent_subagents: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub model: ModelSelectorSpec,
    pub context: ContextSpec,
    pub reasoning: ReasoningSpec,
    pub tools: ToolsSpec,
    pub cache: CacheSpec,
    #[serde(default)]
    pub sampling: Option<SamplingSpec>,
    pub limits: LimitsSpec,
}

impl ModelProfile {
    pub fn from_toml_str(input: &str) -> Result<Self, SchemaError> {
        let profile: Self = toml::from_str(input)?;
        profile.validate()?;
        Ok(profile)
    }

    pub fn validate(&self) -> Result<(), SchemaError> {
        if self.context.soft_shake_tokens.is_none() && self.context.soft_shake_ratio.is_none() {
            return Err(SchemaError::Validation(
                "context must specify either soft_shake_tokens or soft_shake_ratio".to_string(),
            ));
        }
        if let Some(r) = self.context.soft_shake_ratio
            && !(0.1..=0.95).contains(&r)
        {
            return Err(SchemaError::Validation(format!(
                "soft_shake_ratio out of bounds: {r}"
            )));
        }
        if self.limits.doom_loop_repeat == 0 {
            return Err(SchemaError::Validation(
                "limits.doom_loop_repeat must be >= 1".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleRoutingSpec {
    pub primary: String,
    pub deep: String,
    pub compact: String,
    pub vision: String,
    pub smol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteDescriptor {
    pub protocol: String,
    pub base_url: String,
    pub models_endpoint: String,
    pub api_key_env: String,
    #[serde(default)]
    pub prefix_cache_engine: Option<String>,
    #[serde(default)]
    pub context_editing_strategy: Option<String>,
    #[serde(default)]
    pub beta_headers: Vec<String>,
    #[serde(default)]
    pub clear_tool_uses_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutesConfig {
    pub roles: RoleRoutingSpec,
    pub routes: BTreeMap<String, RouteDescriptor>,
}

impl RoutesConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, SchemaError> {
        let cfg: Self = toml::from_str(input)?;
        Ok(cfg)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModel {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub native_window: u64,
    pub effective_window: u64,
    pub max_output_tokens: u64,
    pub supports_adaptive_thinking: bool,
    pub supports_vision: bool,
    pub resolution_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub display_name: String,
    pub default_profile: String,
}
