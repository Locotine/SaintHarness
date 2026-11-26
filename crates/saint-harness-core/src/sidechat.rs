use crate::context::{RequestBuildInput, build_llm_request};
use crate::store::EventStore;
use futures::StreamExt;
use saint_harness_schema::{
    ContentPart, LLMEvent, Message, ModelProfile, ResolvedModel, Role, RouteDescriptor, ToolChoice,
    WireProtocol,
};
use std::sync::Arc;

/// Executes an ephemeral `/btw` sidechat question borrowing the session's warm prefix cache.
/// Strictly read-only: NEVER writes to `session_message` or mutates session state.
#[allow(clippy::too_many_arguments)]
pub async fn run_btw_sidechat(
    store: &EventStore,
    protocol: Arc<dyn WireProtocol>,
    session_id: &str,
    system_blocks: Vec<String>,
    model: ResolvedModel,
    profile: ModelProfile,
    route: RouteDescriptor,
    question: &str,
) -> Result<String, String> {
    let chain = store.reader.walk_active_branch(session_id)?;
    let mut history: Vec<Message> = chain.into_iter().map(|r| r.message).collect();
    history.push(Message {
        id: "btw_ephemeral".to_string(),
        parent_msg_id: None,
        role: Role::User,
        parts: vec![ContentPart::Text {
            text: format!("[/btw side question - answer concisely without tools]\n{question}"),
            provider_meta: None,
        }],
        model_provider: None,
        model_id: None,
    });

    let req = build_llm_request(RequestBuildInput {
        system: system_blocks,
        history: &history,
        tools: vec![],
        tool_choice: ToolChoice::None,
        model,
        profile,
        route,
        measured_input_tokens: 0,
        estimated_delta_tokens: 0,
        session_id,
        parent_id: None,
        fork_source_id: None,
        smol_mode: true,
    });

    let mut stream = protocol.stream(req).await.map_err(|e| e.to_string())?;
    let mut answer = String::new();
    while let Some(item) = stream.next().await {
        if let Ok(LLMEvent::TextDelta { delta }) = item {
            answer.push_str(&delta);
        }
    }
    Ok(answer)
}
