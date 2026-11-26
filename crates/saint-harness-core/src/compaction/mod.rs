use crate::store::BackgroundJobRow;
use saint_harness_schema::{ContentPart, Message, Role, embedded_prompt};
use saint_harness_token::{SHRINK_STEPS, flatten_and_drop_oldest, run_tier1_shake};
use std::path::Path;

pub const REQUIRED_COMPACTION_HEADINGS: [&str; 7] = [
    "## 1. Objective & Acceptance Criteria",
    "## 2. Workspace State & Modified Files",
    "## 3. Verified Facts & Architecture Decisions",
    "## 4. Test & Verification Results",
    "## 5. Pending Work & Next Concrete Step",
    "## 6. Open Questions, Risks & User Preferences",
    "## 7. Active Subagents & Background Work",
];

#[derive(Debug, Clone)]
pub struct PrecomputedSummary {
    pub held_index: usize,
    pub summary_markdown: String,
    pub consecutive_failures: usize,
}

#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub tier_used: u8,
    pub messages: Vec<Message>,
    pub summary_xml: Option<String>,
    pub nudged: bool,
}

pub fn missing_compaction_headings(summary: &str) -> Vec<&'static str> {
    REQUIRED_COMPACTION_HEADINGS
        .iter()
        .copied()
        .filter(|h| !summary.contains(h))
        .collect()
}

pub fn build_compaction_prompt(
    existing_summary: Option<&str>,
    background_jobs: &[BackgroundJobRow],
    active_subagents: &[String],
) -> String {
    let base = embedded_prompt("prompts/compaction.txt").unwrap_or("Summarize the conversation.");
    let mut prompt = String::from(base);
    if let Some(prev) = existing_summary {
        prompt.push_str("\n\n<previous-summary>\n");
        prompt.push_str(prev);
        prompt.push_str("\n</previous-summary>");
    }
    if !active_subagents.is_empty() {
        prompt.push_str("\n\n<subagents>\n");
        for s in active_subagents {
            prompt.push_str(&format!("- {s}\n"));
        }
        prompt.push_str("</subagents>");
    }
    if !background_jobs.is_empty() {
        prompt.push_str("\n\n<background-work>\n");
        for j in background_jobs {
            prompt.push_str(&format!(
                "- [{}] {} ({}) exit={:?}\n",
                j.kind, j.command_preview, j.status, j.exit_code
            ));
        }
        prompt.push_str("</background-work>");
    }
    prompt
}

/// Executes the 5-tier compaction orchestrator:
/// - Tier 1: Zero-LLM `shake` (spills bulky blocks to zstd); stops immediately if `< 55%` ceiling.
/// - Tier 4: Structured 7-section summary using `summarizer` callback with 1x NUDGE if headings are missing
///   and `SHRINK_STEPS` (`0.85 -> 0.70 -> 0.50 -> 0.35`) overflow backoff.
#[allow(clippy::too_many_arguments)]
pub async fn run_compaction<F, Fut>(
    messages: &[Message],
    ceiling_tokens: u64,
    keep_recent_messages: usize,
    spill_dir: Option<&Path>,
    background_jobs: &[BackgroundJobRow],
    active_subagents: &[String],
    precomputed: Option<&PrecomputedSummary>,
    summarizer: F,
) -> Result<CompactionResult, String>
where
    F: Fn(Vec<Message>, String) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    // Tier 1: Zero-LLM shake
    let shake = run_tier1_shake(messages, ceiling_tokens, keep_recent_messages, spill_dir);
    if shake.below_target {
        return Ok(CompactionResult {
            tier_used: 1,
            messages: shake.messages,
            summary_xml: None,
            nudged: false,
        });
    }

    // Check reusable Background Precompute summary when tail is <= 40% of total messages.
    if let Some(pre) = precomputed {
        let tail_len = messages.len().saturating_sub(pre.held_index);
        if pre.held_index > 0
            && pre.held_index <= messages.len()
            && tail_len * 5 <= messages.len() * 2
        {
            let mut out = vec![compaction_message(&pre.summary_markdown)];
            out.extend_from_slice(&messages[pre.held_index..]);
            return Ok(CompactionResult {
                tier_used: 4,
                messages: out,
                summary_xml: Some(pre.summary_markdown.clone()),
                nudged: false,
            });
        }
    }

    // Tier 4: Structured 7-section summary with SHRINK_STEPS retry on overflow + 1x NUDGE.
    let base_prompt = build_compaction_prompt(None, background_jobs, active_subagents);
    let mut summary_text = None;
    for ratio in SHRINK_STEPS {
        let target = ((ceiling_tokens as f64) * ratio).floor() as u64;
        let reduced = flatten_and_drop_oldest(&shake.messages, target);
        match summarizer(reduced.messages, base_prompt.clone()).await {
            Ok(text) => {
                summary_text = Some(text);
                break;
            }
            Err(e) if e.to_ascii_lowercase().contains("overflow") => continue,
            Err(e) => return Err(e),
        }
    }

    let mut summary = summary_text
        .ok_or_else(|| "compaction failed after exhausting all SHRINK_STEPS".to_string())?;
    let mut nudged = false;
    let missing = missing_compaction_headings(&summary);
    if !missing.is_empty() {
        nudged = true;
        let nudge_prompt = format!(
            "Your summary omitted required headings: {}. Rewrite preserving all facts and include all 7 exact headings.\n\nDraft:\n{}",
            missing.join(", "),
            summary
        );
        if let Ok(fixed) = summarizer(vec![], nudge_prompt).await {
            summary = fixed;
        }
    }

    let tail_start = messages.len().saturating_sub(keep_recent_messages.min(4));
    let mut compacted = vec![compaction_message(&summary)];
    compacted.extend_from_slice(&messages[tail_start..]);

    Ok(CompactionResult {
        tier_used: 4,
        messages: compacted,
        summary_xml: Some(summary),
        nudged,
    })
}

fn compaction_message(summary: &str) -> Message {
    Message {
        id: format!("cmp_{}", ulid::Ulid::new()),
        parent_msg_id: None,
        role: Role::Compaction,
        parts: vec![ContentPart::Compaction {
            encrypted: None,
            summary_xml: Some(format!(
                "<compaction-summary>\n{summary}\n</compaction-summary>"
            )),
        }],
        model_provider: None,
        model_id: None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn compaction_nudges_once_when_headings_missing_and_retries_shrink_steps() {
        let msgs: Vec<Message> = (0..6)
            .map(|i| Message {
                id: format!("m{i}"),
                parent_msg_id: None,
                role: Role::User,
                parts: vec![ContentPart::Text {
                    text: "detailed context block ".repeat(400),
                    provider_meta: None,
                }],
                model_provider: None,
                model_id: None,
            })
            .collect();

        let calls = AtomicUsize::new(0);
        let res = run_compaction(
            &msgs,
            3_000,
            2,
            None,
            &[],
            &[],
            None,
            |_reduced, _prompt| {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if n == 0 {
                        Err("context overflow".to_string())
                    } else if n == 1 {
                        Ok("## 1. Objective & Acceptance Criteria\nIncomplete draft".to_string())
                    } else {
                        Ok(REQUIRED_COMPACTION_HEADINGS.join("\nDetails\n"))
                    }
                }
            },
        )
        .await
        .unwrap();

        assert_eq!(res.tier_used, 4);
        assert!(res.nudged);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(missing_compaction_headings(res.summary_xml.as_deref().unwrap()).is_empty());
    }
}
