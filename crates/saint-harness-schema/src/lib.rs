pub mod agent;
pub mod error;
pub mod event;
pub mod guard;
pub mod mcp;
pub mod message;
pub mod permission;
pub mod ports;
pub mod settings;
pub mod tool;

pub use agent::*;
pub use error::*;
pub use event::*;
pub use guard::*;
pub use mcp::*;
pub use message::*;
pub use permission::*;
pub use ports::*;
pub use settings::*;
pub use tool::*;

pub const EMBEDDED_PROFILES: &[(&str, &str)] = &[
    (
        "qwen-3.8-27b",
        include_str!("../../../profiles/qwen-3.8-27b.toml"),
    ),
    (
        "qwen-4-upcoming",
        include_str!("../../../profiles/qwen-4-upcoming.toml"),
    ),
    (
        "claude-flagship",
        include_str!("../../../profiles/claude-flagship.toml"),
    ),
    (
        "gemini-flagship",
        include_str!("../../../profiles/gemini-flagship.toml"),
    ),
];

pub const EMBEDDED_ROUTES_TOML: &str = include_str!("../../../profiles/routes.toml");

pub const EMBEDDED_PROMPTS: &[(&str, &str)] = &[
    (
        "prompts/base.txt",
        include_str!("../../../prompts/base.txt"),
    ),
    (
        "prompts/qwen-3.8.txt",
        include_str!("../../../prompts/qwen-3.8.txt"),
    ),
    (
        "prompts/qwen-4.txt",
        include_str!("../../../prompts/qwen-4.txt"),
    ),
    (
        "prompts/claude.txt",
        include_str!("../../../prompts/claude.txt"),
    ),
    (
        "prompts/gemini.txt",
        include_str!("../../../prompts/gemini.txt"),
    ),
    (
        "prompts/compaction.txt",
        include_str!("../../../prompts/compaction.txt"),
    ),
];

pub fn embedded_profile(name: &str) -> Result<ModelProfile, SchemaError> {
    let raw = EMBEDDED_PROFILES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
        .ok_or_else(|| SchemaError::UnknownProfile(name.to_string()))?;
    ModelProfile::from_toml_str(raw)
}

pub fn embedded_routes() -> Result<RoutesConfig, SchemaError> {
    RoutesConfig::from_toml_str(EMBEDDED_ROUTES_TOML)
}

pub fn embedded_prompt(path: &str) -> Option<&'static str> {
    EMBEDDED_PROMPTS
        .iter()
        .find(|(k, _)| *k == path)
        .map(|(_, v)| *v)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn all_embedded_profiles_and_routes_deserialize_strictly() {
        for (name, raw) in EMBEDDED_PROFILES {
            let profile = ModelProfile::from_toml_str(raw)
                .unwrap_or_else(|e| panic!("failed to parse {name}: {e}"));
            assert!(
                profile.validate().is_ok(),
                "profile {name} failed validation"
            );
        }
        let routes = embedded_routes().expect("routes.toml must deserialize");
        assert_eq!(routes.roles.primary, "qwen-3.8-27b");
        assert!(routes.routes.contains_key("anthropic_direct"));
        assert!(routes.routes.contains_key("google_ai_studio"));
        assert!(routes.routes.contains_key("qwen_local_sglang"));
    }

    #[test]
    fn usage_saturating_and_cache_ratio() {
        let u = Usage {
            input_tokens: 1000,
            non_cached_input_tokens: 200,
            cache_read_input_tokens: 800,
            cache_write_input_tokens: 0,
            output_tokens: 50,
            reasoning_tokens: 80,
            provider_reported_cost_usd: None,
        };
        assert_eq!(u.visible_output_tokens(), 0);
        assert!((u.cache_hit_ratio() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn permission_effect_lattice_ordering() {
        assert!(PermissionEffect::Allow < PermissionEffect::Ask);
        assert!(PermissionEffect::Ask < PermissionEffect::Deny);
        assert_eq!(
            PermissionEffect::Allow.max(PermissionEffect::Deny),
            PermissionEffect::Deny
        );
    }

    #[test]
    fn secret_string_is_redacted_in_debug_display_and_json() {
        let s = SecretString::new("sk-live-super-secret-value");
        assert!(!format!("{s:?}").contains("super-secret"));
        assert!(!format!("{s}").contains("super-secret"));
        let spec = McpServerSpec {
            name: "saint".to_string(),
            transport: McpTransport::StreamableHttp {
                url: "http://127.0.0.1:9999/mcp".to_string(),
                bearer_token: Some(s),
                headers: Default::default(),
            },
            exposure: McpExposure::Direct,
            auto_approve: true,
        };
        let serialized = serde_json::to_string(&spec).unwrap();
        assert!(!serialized.contains("super-secret"));
    }

    #[tokio::test]
    async fn noop_policy_guard_always_passes() {
        let guard = NoopPolicyGuard;
        assert!(matches!(
            guard.inspect_input("s1", "hello").await,
            GuardDecision::Pass
        ));
        assert!(matches!(
            guard
                .inspect_tool_call("shell", &serde_json::json!({"cmd": "ls"}))
                .await,
            GuardDecision::Pass
        ));
        assert!(matches!(
            guard.inspect_output("s1", "done").await,
            GuardDecision::Pass
        ));
    }

    #[test]
    fn message_and_event_json_roundtrip() {
        let msg = Message {
            id: "msg_1".to_string(),
            parent_msg_id: None,
            role: Role::Assistant,
            parts: vec![
                ContentPart::Reasoning {
                    text: "plan".to_string(),
                    signature: Some("sig1".to_string()),
                    redacted_data: None,
                    provider_meta: None,
                },
                ContentPart::ToolCall {
                    id: "call_1".to_string(),
                    name: "read".to_string(),
                    arguments: serde_json::json!({"path": "Cargo.toml"}),
                    provider_executed: false,
                    signature: None,
                },
            ],
            model_provider: Some("qwen".to_string()),
            model_id: Some("qwen3.8-27b".to_string()),
        };
        let encoded = serde_json::to_string(&msg).unwrap();
        let decoded: Message = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.id, "msg_1");
        assert_eq!(decoded.parts.len(), 2);
    }
}
