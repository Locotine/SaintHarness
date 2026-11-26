use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
    System,
    Synthetic,
    Compaction,
    Tool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub non_cached_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub provider_reported_cost_usd: Option<f64>,
}

impl Usage {
    pub fn visible_output_tokens(&self) -> u64 {
        self.output_tokens.saturating_sub(self.reasoning_tokens)
    }

    pub fn cache_hit_ratio(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cache_read_input_tokens as f64 / self.input_tokens as f64
        }
    }

    pub fn saturating_add(&self, other: &Usage) -> Usage {
        let cost = match (
            self.provider_reported_cost_usd,
            other.provider_reported_cost_usd,
        ) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        Usage {
            input_tokens: self.input_tokens.saturating_add(other.input_tokens),
            non_cached_input_tokens: self
                .non_cached_input_tokens
                .saturating_add(other.non_cached_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_add(other.cache_read_input_tokens),
            cache_write_input_tokens: self
                .cache_write_input_tokens
                .saturating_add(other.cache_write_input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            reasoning_tokens: self.reasoning_tokens.saturating_add(other.reasoning_tokens),
            provider_reported_cost_usd: cost,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultBlock {
    Text {
        text: String,
    },
    Media {
        mime_type: String,
        base64_data: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_meta: Option<BTreeMap<String, Value>>,
    },
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        redacted_data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_meta: Option<BTreeMap<String, Value>>,
    },
    Media {
        mime_type: String,
        base64_data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_path: Option<String>,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: Value,
        #[serde(default)]
        provider_executed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolResult {
        call_id: String,
        name: String,
        content: Vec<ToolResultBlock>,
        #[serde(default)]
        is_error: bool,
    },
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_xml: Option<String>,
    },
    EffortMarker {
        effort: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_msg_id: Option<String>,
    pub role: Role,
    pub parts: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

impl Message {
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for part in &self.parts {
            if let ContentPart::Text { text, .. } = part {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
        out
    }
}
