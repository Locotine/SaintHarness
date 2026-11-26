use crate::cache_meter::CacheMeter;
use crate::estimator::estimate_part_tokens;
use chrono::{DateTime, Utc};
use saint_harness_schema::{ContentPart, Message, ToolResultBlock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpochAdvanceReason {
    CacheCold,
    SoftShake,
    BreakEven,
    Compaction,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PruneEpochSnapshot {
    pub epoch_id: u64,
    pub reason: Option<EpochAdvanceReason>,
    pub pruned_map: BTreeMap<String, String>,
}

/// Accumulates superseded reads and old tool observations while cache is Warm,
/// only advancing `PruneEpochSnapshot` when cold, hitting `soft_shake`, breaking even, or compacting.
#[derive(Debug, Clone, Default)]
pub struct PendingPruneTracker {
    pub active_epoch: PruneEpochSnapshot,
    pending_masks: BTreeMap<String, (String, u64)>,
}

impl PendingPruneTracker {
    pub fn pending_count(&self) -> usize {
        self.pending_masks.len()
    }

    pub fn pending_reclaimable_tokens(&self) -> u64 {
        self.pending_masks.values().map(|(_, t)| *t).sum()
    }

    /// Scans conversation history for superseded file reads and observations outside the recent
    /// `keep_recent_turns` / `keep_tail_tokens` window. Preserves Reasoning + ToolCall framing.
    pub fn scan_candidates(
        &mut self,
        messages: &[Message],
        keep_recent_turns: usize,
        keep_tail_tokens: u64,
    ) {
        // 1. Superseded read dedup: if the same path is read multiple times, earlier reads become superseded.
        let mut latest_read_by_path: HashMap<String, String> = HashMap::new();
        let mut read_call_paths: HashMap<String, String> = HashMap::new();
        for msg in messages {
            for part in &msg.parts {
                if let ContentPart::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } = part
                    && name == "read"
                    && let Some(path) = arguments.get("path").and_then(|v| v.as_str())
                {
                    if let Some(prev_id) = latest_read_by_path.insert(path.to_string(), id.clone())
                    {
                        let prev_path = read_call_paths
                            .get(&prev_id)
                            .cloned()
                            .unwrap_or_else(|| path.to_string());
                        self.pending_masks
                            .entry(prev_id)
                            .or_insert_with(|| (format!("[superseded read of {prev_path}]"), 200));
                    }
                    read_call_paths.insert(id.clone(), path.to_string());
                }
            }
        }

        // 2. Observation masking beyond recent turn & token tail window.
        let mut tail_tokens = 0u64;
        let mut protected_from = messages.len();
        for (idx, msg) in messages.iter().enumerate().rev() {
            let msg_tok: u64 = msg.parts.iter().map(estimate_part_tokens).sum();
            let turns_from_end = messages.len().saturating_sub(idx);
            if turns_from_end <= keep_recent_turns || tail_tokens < keep_tail_tokens {
                tail_tokens = tail_tokens.saturating_add(msg_tok);
                protected_from = idx;
            } else {
                break;
            }
        }

        for msg in &messages[..protected_from] {
            for part in &msg.parts {
                if let ContentPart::ToolResult {
                    call_id,
                    name,
                    content,
                    ..
                } = part
                {
                    if self.active_epoch.pruned_map.contains_key(call_id) {
                        continue;
                    }
                    let tok = estimate_part_tokens(part);
                    if tok >= 120 {
                        let preview = content
                            .iter()
                            .find_map(|b| match b {
                                ToolResultBlock::Text { text } => Some(
                                    text.lines()
                                        .next()
                                        .unwrap_or("")
                                        .chars()
                                        .take(60)
                                        .collect::<String>(),
                                ),
                                _ => None,
                            })
                            .unwrap_or_default();
                        self.pending_masks.insert(
                            call_id.clone(),
                            (
                                format!("[masked {name} result (~{tok} tok): {preview}]"),
                                tok,
                            ),
                        );
                    }
                }
            }
        }
    }

    pub fn maybe_advance_epoch(
        &mut self,
        meter: &CacheMeter,
        now: DateTime<Utc>,
        estimated_tokens: u64,
        soft_shake_tokens: u64,
        force_compaction: bool,
    ) -> Option<EpochAdvanceReason> {
        if self.pending_masks.is_empty() && !force_compaction {
            return None;
        }
        let reason = if force_compaction {
            Some(EpochAdvanceReason::Compaction)
        } else if !meter.is_warm(now) {
            Some(EpochAdvanceReason::CacheCold)
        } else if estimated_tokens >= soft_shake_tokens {
            Some(EpochAdvanceReason::SoftShake)
        } else if meter.break_even_saving(self.pending_reclaimable_tokens(), estimated_tokens, 6)
            > 0
        {
            Some(EpochAdvanceReason::BreakEven)
        } else {
            None
        }?;

        self.active_epoch.epoch_id += 1;
        self.active_epoch.reason = Some(reason);
        for (call_id, (placeholder, _)) in std::mem::take(&mut self.pending_masks) {
            self.active_epoch.pruned_map.insert(call_id, placeholder);
        }
        Some(reason)
    }

    /// Applies the frozen epoch map to messages without altering unmasked tool calls or reasoning.
    pub fn apply_frozen_view(&self, messages: &[Message]) -> Vec<Message> {
        if self.active_epoch.pruned_map.is_empty() {
            return messages.to_vec();
        }
        messages
            .iter()
            .map(|msg| {
                let mut cloned = msg.clone();
                cloned.parts = cloned
                    .parts
                    .into_iter()
                    .map(|part| match part {
                        ContentPart::ToolResult {
                            call_id,
                            name,
                            content,
                            is_error,
                        } => {
                            if let Some(placeholder) = self.active_epoch.pruned_map.get(&call_id) {
                                ContentPart::ToolResult {
                                    call_id,
                                    name,
                                    content: vec![ToolResultBlock::Text {
                                        text: placeholder.clone(),
                                    }],
                                    is_error,
                                }
                            } else {
                                ContentPart::ToolResult {
                                    call_id,
                                    name,
                                    content,
                                    is_error,
                                }
                            }
                        }
                        other => other,
                    })
                    .collect();
                cloned
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Duration;
    use saint_harness_schema::{Role, Usage};
    use serde_json::json;

    fn make_tool_exchange(i: usize, body_size: usize) -> Vec<Message> {
        vec![
            Message {
                id: format!("a_{i}"),
                parent_msg_id: None,
                role: Role::Assistant,
                parts: vec![
                    ContentPart::Reasoning {
                        text: format!("checking {i}"),
                        signature: None,
                        redacted_data: None,
                        provider_meta: None,
                    },
                    ContentPart::ToolCall {
                        id: format!("c_{i}"),
                        name: "read".into(),
                        arguments: json!({"path": "src/lib.rs"}),
                        provider_executed: false,
                        signature: None,
                    },
                ],
                model_provider: None,
                model_id: None,
            },
            Message {
                id: format!("t_{i}"),
                parent_msg_id: None,
                role: Role::Tool,
                parts: vec![ContentPart::ToolResult {
                    call_id: format!("c_{i}"),
                    name: "read".into(),
                    content: vec![ToolResultBlock::Text {
                        text: "fn sample() {}\n".repeat(body_size),
                    }],
                    is_error: false,
                }],
                model_provider: None,
                model_id: None,
            },
        ]
    }

    #[test]
    fn prefix_determinism_freezes_during_warm_and_flushes_on_cold_ttl() {
        let mut msgs = Vec::new();
        for i in 0..12 {
            msgs.extend(make_tool_exchange(i, 60));
        }

        let t0 = Utc::now();
        let mut meter = CacheMeter::new(300);
        let warm_usage = Usage {
            input_tokens: 10_000,
            non_cached_input_tokens: 1_000,
            cache_read_input_tokens: 9_000,
            cache_write_input_tokens: 0,
            output_tokens: 100,
            reasoning_tokens: 0,
            provider_reported_cost_usd: None,
        };
        meter.record_step("prefix", &warm_usage, t0);

        let mut tracker = PendingPruneTracker::default();
        tracker.scan_candidates(&msgs, 4, 2_000);
        assert!(tracker.pending_count() > 0);

        // 4 consecutive warm turns: epoch MUST NOT advance and prefix hash stays 100% identical.
        let mut hashes = Vec::new();
        for turn in 1..=4 {
            let now = t0 + Duration::seconds(turn * 30);
            assert_eq!(
                tracker.maybe_advance_epoch(&meter, now, 20_000, 62_000, false),
                None
            );
            let view = tracker.apply_frozen_view(&msgs);
            let h = blake3::hash(serde_json::to_string(&view).unwrap().as_bytes())
                .to_hex()
                .to_string();
            hashes.push(h);
        }
        assert!(hashes.windows(2).all(|w| w[0] == w[1]));

        // Turn 5: TTL expired -> cache cold -> epoch advances and masks old observations.
        let cold_now = t0 + Duration::seconds(360);
        assert_eq!(
            tracker.maybe_advance_epoch(&meter, cold_now, 20_000, 62_000, false),
            Some(EpochAdvanceReason::CacheCold)
        );
        assert_eq!(tracker.active_epoch.epoch_id, 1);
        let masked_view = tracker.apply_frozen_view(&msgs);
        let serialized = serde_json::to_string(&masked_view).unwrap();
        assert!(serialized.contains("[superseded read") || serialized.contains("[masked read"));
        // Reasoning & ToolCall skeleton remain intact
        assert!(serialized.contains("checking 0"));
    }
}
