pub mod request_builder;
pub mod to_llm;

pub use request_builder::*;
pub use to_llm::*;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::{ContentPart, Message, Role};
    use serde_json::json;

    #[test]
    fn six_pair_cross_model_handoff_matrix_and_image_hysteresis() {
        let hist = vec![Message {
            id: "a1".into(),
            parent_msg_id: None,
            role: Role::Assistant,
            parts: vec![
                ContentPart::Reasoning {
                    text: "deep thought".into(),
                    signature: Some("sig_claude".into()),
                    redacted_data: None,
                    provider_meta: None,
                },
                ContentPart::ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: json!({"path": "a.rs"}),
                    provider_executed: false,
                    signature: None,
                },
            ],
            model_provider: Some("anthropic".into()),
            model_id: Some("claude-a".into()),
        }];

        // Claude -> Gemini attaches sentinel and strips foreign thinking
        let to_gem = to_llm_messages(&hist, "google", "gemini-b");
        assert!(matches!(
            &to_gem[0].parts[0],
            ContentPart::ToolCall { signature: Some(s), .. } if s == GEMINI_UNSIGNED_SENTINEL
        ));

        // Claude -> Claude same model keeps thinking + signature
        let same_claude = to_llm_messages(&hist, "anthropic", "claude-a");
        assert!(matches!(
            &same_claude[0].parts[0],
            ContentPart::Reasoning { signature: Some(s), .. } if s == "sig_claude"
        ));

        // Claude -> different Claude drops foreign thinking signature
        let other_claude = to_llm_messages(&hist, "anthropic", "claude-b");
        assert_eq!(other_claude[0].parts.len(), 1);

        // Claude -> Qwen strips all provider metadata/thinking
        let to_qwen = to_llm_messages(&hist, "qwen", "qwen3.8-27b");
        assert_eq!(to_qwen[0].parts.len(), 1);
        assert!(matches!(
            &to_qwen[0].parts[0],
            ContentPart::ToolCall {
                signature: None,
                ..
            }
        ));

        // bound_images hysteresis: stays intact under high watermark, prunes to low watermark once exceeded
        let mut imgs = vec![
            Message {
                id: "u1".into(),
                parent_msg_id: None,
                role: Role::User,
                parts: vec![ContentPart::Media {
                    mime_type: "image/png".into(),
                    base64_data: "A".repeat(600),
                    source_path: Some("1.png".into()),
                }],
                model_provider: None,
                model_id: None,
            },
            Message {
                id: "u2".into(),
                parent_msg_id: None,
                role: Role::User,
                parts: vec![ContentPart::Media {
                    mime_type: "image/png".into(),
                    base64_data: "B".repeat(600),
                    source_path: Some("2.png".into()),
                }],
                model_provider: None,
                model_id: None,
            },
        ];
        bound_images_hysteresis(&mut imgs, 1000, 650);
        assert!(matches!(imgs[0].parts[0], ContentPart::Text { .. }));
        assert!(matches!(imgs[1].parts[0], ContentPart::Media { .. }));
    }
}
