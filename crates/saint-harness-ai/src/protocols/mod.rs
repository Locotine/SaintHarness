pub mod anthropic;
pub mod external_stdio;
pub mod gemini;
pub mod openai_chat;
pub mod qwen_guard;

use saint_harness_schema::WireProtocol;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ProtocolRegistry {
    protocols: BTreeMap<String, Arc<dyn WireProtocol>>,
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        let mut reg = Self {
            protocols: BTreeMap::new(),
        };
        reg.register(Arc::new(openai_chat::OpenAiChatCompatProtocol::default()));
        reg.register(Arc::new(anthropic::AnthropicMessagesProtocol::default()));
        reg.register(Arc::new(gemini::GeminiGenerateContentProtocol::default()));
        reg
    }
}

impl ProtocolRegistry {
    pub fn register(&mut self, proto: Arc<dyn WireProtocol>) {
        self.protocols
            .insert(proto.protocol_id().to_string(), proto);
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn WireProtocol>> {
        self.protocols.get(id).cloned()
    }
}
