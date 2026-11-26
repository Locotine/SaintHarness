use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicyKind {
    AnthropicInline4,
    QwenSystemTail,
    ImplicitPrefix,
    GeminiImplicit,
}

impl CachePolicyKind {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "anthropic_inline4" => Self::AnthropicInline4,
            "qwen_system_tail" => Self::QwenSystemTail,
            "gemini_implicit" => Self::GeminiImplicit,
            _ => Self::ImplicitPrefix,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheBreakpointPlan {
    pub policy: CachePolicyKind,
    pub mark_last_tool: bool,
    pub mark_system_indices: Vec<usize>,
    pub mark_tail_message: bool,
}

impl CacheBreakpointPlan {
    pub fn total_breakpoints(&self) -> usize {
        usize::from(self.mark_last_tool)
            + self.mark_system_indices.len()
            + usize::from(self.mark_tail_message)
    }
}

/// Computes provider cache breakpoint placement while strictly respecting Anthropic's 4-breakpoint cap.
pub fn plan_cache_breakpoints(
    policy: CachePolicyKind,
    tool_count: usize,
    system_count: usize,
    message_count: usize,
) -> CacheBreakpointPlan {
    match policy {
        CachePolicyKind::AnthropicInline4 => {
            let mut budget = 4usize;
            let mark_last_tool = if tool_count > 0 && budget > 0 {
                budget -= 1;
                true
            } else {
                false
            };
            let mut mark_system_indices = Vec::new();
            for idx in 0..system_count.min(2) {
                if budget > 0 {
                    mark_system_indices.push(idx);
                    budget -= 1;
                }
            }
            let mark_tail_message = message_count > 0 && budget > 0;
            CacheBreakpointPlan {
                policy,
                mark_last_tool,
                mark_system_indices,
                mark_tail_message,
            }
        }
        CachePolicyKind::QwenSystemTail => CacheBreakpointPlan {
            policy,
            mark_last_tool: false,
            mark_system_indices: if system_count > 0 {
                vec![system_count - 1]
            } else {
                Vec::new()
            },
            mark_tail_message: message_count > 0,
        },
        CachePolicyKind::ImplicitPrefix | CachePolicyKind::GeminiImplicit => CacheBreakpointPlan {
            policy,
            mark_last_tool: false,
            mark_system_indices: Vec::new(),
            mark_tail_message: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_inline4_never_exceeds_four_breakpoints() {
        let plan = plan_cache_breakpoints(CachePolicyKind::AnthropicInline4, 12, 5, 20);
        assert!(plan.total_breakpoints() <= 4);
        assert_eq!(plan.total_breakpoints(), 4);
        assert!(plan.mark_last_tool);
        assert_eq!(plan.mark_system_indices, vec![0, 1]);
        assert!(plan.mark_tail_message);
    }

    #[test]
    fn gemini_implicit_places_zero_inline_breakpoints() {
        let plan = plan_cache_breakpoints(CachePolicyKind::GeminiImplicit, 10, 2, 10);
        assert_eq!(plan.total_breakpoints(), 0);
    }
}
