use chrono::Utc;
use globset::Glob;
use imara_diff::{Algorithm, Diff, InternedInput, UnifiedDiffConfig};
use saint_harness_schema::{InteractionMode, ModelProfile, embedded_prompt};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Manages the two stable system blocks (`system[0]`, `system[1]`) and emits compact
/// `imara-diff` `<system-update>` deltas whenever `AGENTS.md`, rules, or `Build <-> Plan` mode changes
/// so the prompt cache prefix stays byte-exact until `advance_epoch()` after compaction.
#[derive(Debug, Clone)]
pub struct InstructionState {
    pub epoch_seq: u64,
    pub system_block_0: String,
    pub system_block_1_frozen: String,
    initial_blobs: BTreeMap<String, String>,
    current_blobs: BTreeMap<String, String>,
    current_mode: InteractionMode,
}

impl InstructionState {
    pub fn initialize(
        workspace_root: &Path,
        profile: &ModelProfile,
        tool_guidance: &str,
        mode: InteractionMode,
        instructions_append: Option<&str>,
    ) -> Self {
        let base = embedded_prompt("prompts/base.txt")
            .unwrap_or("You are SaintHarness.")
            .replace("${TOOL_GUIDANCE}", tool_guidance);
        let tune = embedded_prompt(&profile.model.prompt_tune_file).unwrap_or("");
        let date_only = Utc::now().format("%Y-%m-%d").to_string();

        let mut block0 = format!("{base}\n\n{tune}\n\n[core/date] {date_only}");
        if let Some(extra) = instructions_append
            && !extra.trim().is_empty()
        {
            block0.push_str("\n\n");
            block0.push_str(extra.trim());
        }

        let mut blobs = discover_agents_md(workspace_root);
        blobs.insert(
            "core/agent".to_string(),
            render_mode_instruction(mode).to_string(),
        );

        let block1 = render_blobs(&blobs);
        Self {
            epoch_seq: 0,
            system_block_0: block0,
            system_block_1_frozen: block1,
            initial_blobs: blobs.clone(),
            current_blobs: blobs,
            current_mode: mode,
        }
    }

    pub fn system_blocks(&self) -> Vec<String> {
        vec![
            self.system_block_0.clone(),
            self.system_block_1_frozen.clone(),
        ]
    }

    /// Re-scans `AGENTS.md` and returns a compact `<system-update>` unified diff without touching `system[1]`.
    pub fn poll_workspace_delta(&mut self, workspace_root: &Path) -> Option<String> {
        let fresh = discover_agents_md(workspace_root);
        let mut deltas = Vec::new();
        for (key, new_text) in fresh {
            let old_text = self.current_blobs.get(&key).cloned().unwrap_or_default();
            if blake3::hash(old_text.as_bytes()) != blake3::hash(new_text.as_bytes()) {
                let input = InternedInput::new(old_text.as_str(), new_text.as_str());
                let diff = Diff::compute(Algorithm::Histogram, &input);
                let mut cfg = UnifiedDiffConfig::default();
                cfg.context_len(1);
                let unified = diff
                    .unified_diff(
                        &imara_diff::BasicLineDiffPrinter(&input.interner),
                        cfg,
                        &input,
                    )
                    .to_string();
                deltas.push(format!("[updated:{key}]\n{unified}"));
                self.current_blobs.insert(key, new_text);
            }
        }
        if deltas.is_empty() {
            None
        } else {
            Some(format!(
                "<system-update>\n{}\n</system-update>",
                deltas.join("\n")
            ))
        }
    }

    /// Switches `Build <-> Plan` via `<system-update>` leaving `system[0..1]` completely immutable.
    pub fn switch_mode(&mut self, new_mode: InteractionMode) -> Option<String> {
        if self.current_mode == new_mode {
            return None;
        }
        self.current_mode = new_mode;
        let text = render_mode_instruction(new_mode).to_string();
        self.current_blobs
            .insert("core/agent".to_string(), text.clone());
        Some(format!(
            "<system-update>\n[core/agent]\n{text}\n</system-update>"
        ))
    }

    /// Loads lazy nested `AGENTS.md` or `.saint-harness/rules/*.md` matching `touched_path`.
    pub fn conditional_rules_for_path(
        &mut self,
        workspace_root: &Path,
        touched_path: &Path,
    ) -> Option<String> {
        let mut injected = Vec::new();
        // Check nested AGENTS.md along parent directories between touched_path and workspace_root.
        let mut cursor = touched_path.parent();
        while let Some(dir) = cursor {
            if dir == workspace_root || !dir.starts_with(workspace_root) {
                break;
            }
            let candidate = dir.join("AGENTS.md");
            if candidate.is_file() {
                let key = candidate
                    .strip_prefix(workspace_root)
                    .unwrap_or(&candidate)
                    .to_string_lossy()
                    .replace('\\', "/");
                if !self.current_blobs.contains_key(&key)
                    && let Ok(body) = fs::read_to_string(&candidate)
                {
                    self.current_blobs.insert(key.clone(), body.clone());
                    injected.push(format!("[nested:{key}]\n{body}"));
                }
            }
            cursor = dir.parent();
        }

        // Check conditional rules under `.saint-harness/rules/*.md` with frontmatter `globs:`.
        let rules_dir = workspace_root.join(".saint-harness").join("rules");
        if let Ok(entries) = fs::read_dir(&rules_dir) {
            let rel_touched = touched_path
                .strip_prefix(workspace_root)
                .unwrap_or(touched_path)
                .to_string_lossy()
                .replace('\\', "/");
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let key = format!(
                    "rule:{}",
                    p.file_name().unwrap_or_default().to_string_lossy()
                );
                if self.current_blobs.contains_key(&key) {
                    continue;
                }
                if let Ok(body) = fs::read_to_string(&p)
                    && rule_matches_path(&body, &rel_touched)
                {
                    self.current_blobs.insert(key.clone(), body.clone());
                    injected.push(format!("[{key}]\n{body}"));
                }
            }
        }

        if injected.is_empty() {
            None
        } else {
            Some(format!(
                "<system-update>\n{}\n</system-update>",
                injected.join("\n")
            ))
        }
    }

    /// Merges all accumulated deltas into `system[1]` cleanly at a Compaction epoch boundary.
    pub fn advance_epoch(&mut self) {
        self.epoch_seq += 1;
        self.initial_blobs = self.current_blobs.clone();
        self.system_block_1_frozen = render_blobs(&self.initial_blobs);
    }
}

fn discover_agents_md(workspace_root: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let root_agents = workspace_root.join("AGENTS.md");
    if root_agents.is_file()
        && let Ok(body) = fs::read_to_string(&root_agents)
    {
        map.insert("AGENTS.md".to_string(), body);
    }
    map
}

fn render_blobs(blobs: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in blobs {
        if k == "core/agent" {
            continue;
        }
        out.push_str(&format!("[{k}]\n{v}\n\n"));
    }
    // Always place `core/agent` last in system[1].
    if let Some(agent) = blobs.get("core/agent") {
        out.push_str(&format!("[core/agent]\n{agent}\n"));
    }
    out
}

fn render_mode_instruction(mode: InteractionMode) -> &'static str {
    match mode {
        InteractionMode::Build => {
            "Active mode: BUILD. You may inspect, edit files, and run workspace verification commands."
        }
        InteractionMode::Plan => {
            "Active mode: PLAN (read-only). Analyze the codebase and propose a plan without modifying files."
        }
    }
}

fn rule_matches_path(body: &str, rel_path: &str) -> bool {
    for line in body.lines().take(10) {
        if let Some(rest) = line.strip_prefix("globs:") {
            for pat in rest.split(',') {
                let trimmed = pat.trim().trim_matches('"').trim_matches('\'');
                if !trimmed.is_empty()
                    && let Ok(g) = Glob::new(trimmed)
                    && g.compile_matcher().is_match(rel_path)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub fn walk_up_agents_files(start: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut cur = Some(start);
    while let Some(dir) = cur {
        let p = dir.join("AGENTS.md");
        if p.is_file() {
            found.push(p);
        }
        cur = dir.parent();
    }
    found.reverse();
    found
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use saint_harness_schema::embedded_profile;

    #[test]
    fn editing_agents_md_and_switching_mode_keep_system1_immutable_until_advance_epoch() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_path = tmp.path().join("AGENTS.md");
        let big = (0..120)
            .map(|i| format!("Rule line {i}: follow workspace guidelines carefully."))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(big.len() > 5000);
        fs::write(&agents_path, &big).unwrap();

        let profile = embedded_profile("qwen-3.8-27b").unwrap();
        let mut state = InstructionState::initialize(
            tmp.path(),
            &profile,
            "tools: read, edit",
            InteractionMode::Build,
            None,
        );
        let orig_sys1 = state.system_block_1_frozen.clone();

        // Modify 1 line in 5KB AGENTS.md -> delta < 400 bytes, system[1] unchanged
        let updated = big.replace(
            "Rule line 50: follow workspace guidelines carefully.",
            "Rule line 50: use strict error hygiene.",
        );
        fs::write(&agents_path, &updated).unwrap();
        let delta = state.poll_workspace_delta(tmp.path()).unwrap();
        assert!(delta.len() < 400, "delta too big: {} bytes", delta.len());
        assert_eq!(state.system_block_1_frozen, orig_sys1);

        // Mode switch Build -> Plan emits <system-update> without mutating system[1]
        let mode_delta = state.switch_mode(InteractionMode::Plan).unwrap();
        assert!(mode_delta.contains("PLAN"));
        assert_eq!(state.system_block_1_frozen, orig_sys1);

        // advance_epoch merges deltas cleanly into system[1]
        state.advance_epoch();
        assert!(state.system_block_1_frozen.contains("strict error hygiene"));
        assert!(state.system_block_1_frozen.contains("PLAN"));
    }
}
