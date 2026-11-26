pub mod mutations;
pub mod read;
pub mod search;

pub use mutations::*;
pub use read::*;
pub use search::*;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::store::EventStore;
    use saint_harness_schema::{
        InteractionMode, RuntimeMode, SandboxProfile, SessionEvent, Tool, ToolContext, ToolOutcome,
        ToolResultBlock,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    #[tokio::test]
    async fn read_delta_reread_binary_reject_fuzzy_and_grep_minified_js() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EventStore::open(tmp.path().join("tools.sqlite")).unwrap();
        store
            .bus
            .publish_durable(
                SessionEvent::SessionCreated {
                    session_id: "s1".into(),
                    parent_id: None,
                    fork_source_id: None,
                    title: "t".into(),
                    agent_id: "build".into(),
                    model_profile: "qwen-3.8-27b".into(),
                    cwd: tmp.path().to_string_lossy().into_owned(),
                    workspace_root: tmp.path().to_string_lossy().into_owned(),
                    runtime_mode: RuntimeMode::Auto,
                    interaction_mode: InteractionMode::Build,
                },
                None,
            )
            .await
            .unwrap();

        let src = tmp.path().join("writer.rs");
        fs::write(&src, "fn one() {}\nfn two() {}\n").unwrap();
        let bin = tmp.path().join("data.bin");
        fs::write(&bin, [0u8, 1, 2, 3]).unwrap();
        let min_js = tmp.path().join("bundle.min.js");
        let huge = format!(
            "{}TARGET_TOKEN_99{}",
            "a".repeat(25_000),
            "b".repeat(25_000)
        );
        fs::write(&min_js, huge).unwrap();

        let ctx = ToolContext {
            session_id: "s1".into(),
            call_id: "c1".into(),
            cwd: tmp.path().to_path_buf(),
            workspace_root: tmp.path().to_path_buf(),
            additional_directories: vec![],
            runtime_mode: RuntimeMode::Auto,
            interaction_mode: InteractionMode::Build,
            sandbox_profile: SandboxProfile::Disabled,
        };

        let read_tool = ReadTool::new(store, None, 200, true, Arc::new(AtomicU64::new(0)));
        // 1st read returns hashline lines
        let r1 = read_tool.execute(&ctx, json!({"path": "writer.rs"})).await;
        assert!(matches!(r1, ToolOutcome::Completed(_)));
        // 2nd read unchanged -> [Unchanged · delta 0B]
        let r2 = read_tool.execute(&ctx, json!({"path": "writer.rs"})).await;
        match r2 {
            ToolOutcome::Completed(tr) => {
                let ToolResultBlock::Text { text } = &tr.content[0] else {
                    panic!("expected text");
                };
                assert!(text.contains("[Unchanged · delta 0B]"));
            }
            _ => panic!("expected completed"),
        }
        // Modify file -> 3rd read returns Delta Re-Read diff
        fs::write(&src, "fn one() {}\nfn two_updated() {}\n").unwrap();
        let r3 = read_tool.execute(&ctx, json!({"path": "writer.rs"})).await;
        match r3 {
            ToolOutcome::Completed(tr) => {
                let ToolResultBlock::Text { text } = &tr.content[0] else {
                    panic!("expected text");
                };
                assert!(text.contains("[Delta Re-Read:"));
                assert!(text.contains("two_updated"));
            }
            _ => panic!("expected completed"),
        }

        // Binary NUL rejected
        let rbin = read_tool.execute(&ctx, json!({"path": "data.bin"})).await;
        assert!(matches!(rbin, ToolOutcome::RecoverableError(_)));

        // Typo triggers nucleo-matcher "Did you mean?"
        let rmiss = read_tool.execute(&ctx, json!({"path": "writr.rs"})).await;
        match rmiss {
            ToolOutcome::RecoverableError(f) => assert!(f.to_string().contains("writer.rs")),
            _ => panic!("expected suggestion error"),
        }

        // Grep on 50KB minified JS clips cleanly to 160-char window
        let grep = GrepTool;
        let rg = grep
            .execute(&ctx, json!({"pattern": "TARGET_TOKEN_99"}))
            .await;
        match rg {
            ToolOutcome::Completed(tr) => {
                let ToolResultBlock::Text { text } = &tr.content[0] else {
                    panic!("expected text");
                };
                assert!(text.contains("TARGET_TOKEN_99"));
                assert!(text.len() < 400);
            }
            _ => panic!("expected grep hit"),
        }
    }
}
