use crate::brand::{LANDING_WAVE_SETTLES_AFTER_MS, SPINNER_FRAMES};
use crate::layout::resolve_layout;
use crate::theme::{SaintTheme, ThemeMode};
use saint_harness_protocol::{
    DecisionCardDto, DiffStatDto, SessionSummaryDto, SessionUiStatus, SseWireEvent, ToolTraceDto,
    TranscriptItemDto, WireInteractionMode, WireRuntimeMode,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonAction {
    SendPrompt {
        session_id: String,
        text: String,
        delivery: String,
    },
    ReplyPermission {
        request_id: String,
        choice: u8,
    },
    Interrupt {
        session_id: String,
    },
}

pub const SLASH_COMMANDS: [(&str, &str); 11] = [
    ("/plan", "Switch to read-only Plan mode (Landlock RO)"),
    (
        "/auto",
        "Enable Auto mode (sandboxed workspace auto-approve)",
    ),
    (
        "/bypass",
        "Enable Bypass permissions / Full access (no prompts, no sandbox)",
    ),
    ("/supervised", "Return to Ask for approval mode"),
    ("/diff", "Toggle Right Split Diff drawer (Ctrl+D)"),
    ("/btw", "Open ephemeral warm-cache sidechat (Ctrl+Alt+S)"),
    ("/tree", "Open Session DAG branch switcher (Ctrl+T)"),
    ("/board", "Switch to Agent Board overview (Ctrl+G)"),
    ("/model", "Switch model / Cross-Model Handoff"),
    ("/compact", "Run Tier 1 zero-LLM shake compaction"),
    ("/cli", "Toggle bottom headless CLI drawer (Ctrl+J)"),
];

pub const COMPOSER_FOOTER_ITEMS: [&str; 4] =
    ["buildPlanMode", "runtimeAccess", "contextUsage", "send"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonConnectionState {
    Ok,
    Loading,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogLoadState {
    Idle,
    Loading,
    Loaded,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    Composer,
    SidebarButton,
    DiffButton,
    ModelButton,
    CacheButton,
    AccessPill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliTab {
    Status,
    Eval,
    Models,
}

#[derive(Debug, Clone)]
pub struct ModelLeafEntry {
    pub key: String,
    pub name: String,
    pub label: String,
    pub selector: String,
    pub tag: String,
    pub handoff_note: String,
}

#[derive(Debug, Clone)]
pub struct ModelProviderNode {
    pub key: String,
    pub label: String,
    pub catalog: String,
    pub tag: String,
    pub handoff_suffix: String,
    pub footnote: Option<String>,
    pub children: Vec<ModelLeafEntry>,
}

#[derive(Debug, Clone)]
pub struct PaletteOption {
    pub key: String,
    pub description: String,
    pub insert_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TreeBranchEntry {
    pub id: String,
    pub prefix: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct DiffLineEntry {
    pub hashline: String,
    pub line_num: String,
    pub sign: char,
    pub code: String,
}

/// Complete interactive TUI state machine matching `prototypes/saint-harness-cli-prototype.html`
/// and `prototypes/tui-parity-contract.json` 1:1.
pub struct TuiApp {
    pub theme: SaintTheme,
    pub reduced_motion: bool,
    pub spinner_idx: usize,
    pub landing_elapsed_ms: u64,
    pub daemon_state: DaemonConnectionState,
    pub daemon_url: String,
    pub live_connected: bool,
    pub pending_actions: Vec<DaemonAction>,
    pub sessions: Vec<SessionSummaryDto>,
    pub active_session_id: String,
    pub sidebar_open: bool,
    pub sidebar_user_toggled: bool,
    pub board_open: bool,
    pub diff_drawer_open: bool,
    pub diff_show_hashline: bool,
    pub diff_active_file: usize,
    pub diff_files: Vec<(String, Vec<DiffLineEntry>)>,
    pub cli_drawer_open: bool,
    pub cli_tab: CliTab,
    pub cli_json_mode: bool,
    pub cache_popover_open: bool,
    pub model_popover_open: bool,
    pub access_popover_open: bool,
    pub btw_open: bool,
    pub btw_input: String,
    pub btw_output: String,
    pub tree_modal_open: bool,
    pub tree_nodes: Vec<TreeBranchEntry>,
    pub tree_head_id: String,
    pub sandbox_details_open: bool,
    pub steer_banner: Option<String>,
    pub composer_input: String,
    pub inline_palette_open: bool,
    pub inline_palette_trigger: Option<char>,
    pub inline_palette_items: Vec<PaletteOption>,
    pub inline_palette_idx: usize,
    pub model_leaves: Vec<ModelLeafEntry>,
    pub model_providers: Vec<ModelProviderNode>,
    pub expanded_providers: BTreeSet<String>,
    pub provider_catalog_state: BTreeMap<String, CatalogLoadState>,
    pub focus: FocusTarget,
    pub last_focus_before_overlay: FocusTarget,
    pub last_user_declined_emitted: bool,
    pub should_quit: bool,
}

impl TuiApp {
    pub fn new(theme_mode: ThemeMode, cols: u16, rows: u16, reduced_motion: bool) -> Self {
        let layout = resolve_layout(cols, rows);
        let sessions = sample_sessions();
        let active_session_id = "done-wal".to_string();

        Self {
            theme: SaintTheme::for_mode(theme_mode),
            reduced_motion,
            spinner_idx: 0,
            landing_elapsed_ms: 0,
            daemon_state: DaemonConnectionState::Ok,
            daemon_url: "http://127.0.0.1:3774".to_string(),
            live_connected: false,
            pending_actions: Vec::new(),
            sessions,
            active_session_id,
            sidebar_open: layout.sidebar_default_open,
            sidebar_user_toggled: false,
            board_open: false,
            diff_drawer_open: false,
            diff_show_hashline: true,
            diff_active_file: 0,
            diff_files: sample_diff_files(),
            cli_drawer_open: false,
            cli_tab: CliTab::Status,
            cli_json_mode: false,
            cache_popover_open: false,
            model_popover_open: false,
            access_popover_open: false,
            btw_open: false,
            btw_input: String::new(),
            btw_output: "Ask a side question on this session's warm cache. The answer is not added to the conversation.".to_string(),
            tree_modal_open: false,
            tree_nodes: vec![
                TreeBranchEntry { id: "msg_001".into(), prefix: String::new(), label: "Fix SQLITE_BUSY under 32 writers".into() },
                TreeBranchEntry { id: "msg_006".into(), prefix: "├─ ".into(), label: "TransactionBehavior::Immediate".into() },
                TreeBranchEntry { id: "msg_014".into(), prefix: "│  └─ ".into(), label: "32-writer test passes ✓".into() },
                TreeBranchEntry { id: "msg_007".into(), prefix: "└─ ".into(), label: "Try r2d2 pool (forked · abandoned)".into() },
            ],
            tree_head_id: "msg_014".to_string(),
            sandbox_details_open: false,
            steer_banner: None,
            composer_input: String::new(),
            inline_palette_open: false,
            inline_palette_trigger: None,
            inline_palette_items: Vec::new(),
            inline_palette_idx: 0,
            model_leaves: default_model_leaves(),
            model_providers: default_model_providers(),
            expanded_providers: BTreeSet::new(),
            provider_catalog_state: BTreeMap::new(),
            focus: FocusTarget::Composer,
            last_focus_before_overlay: FocusTarget::Composer,
            last_user_declined_emitted: false,
            should_quit: false,
        }
    }

    pub fn on_resize(&mut self, cols: u16, rows: u16) {
        if !self.sidebar_user_toggled {
            self.sidebar_open = resolve_layout(cols, rows).sidebar_default_open;
        }
    }

    pub fn tick(&mut self, elapsed_ms: u64) {
        if self.reduced_motion {
            return;
        }
        self.spinner_idx = (self.spinner_idx + 1) % SPINNER_FRAMES.len();
        self.landing_elapsed_ms = self.landing_elapsed_ms.saturating_add(elapsed_ms);
    }

    pub fn is_landing_wave_active(&self) -> bool {
        !self.reduced_motion && self.landing_elapsed_ms < LANDING_WAVE_SETTLES_AFTER_MS
    }

    pub fn spinner_glyph(&self) -> &'static str {
        SPINNER_FRAMES[self.spinner_idx % SPINNER_FRAMES.len()]
    }

    pub fn current_session(&self) -> Option<&SessionSummaryDto> {
        self.sessions
            .iter()
            .find(|s| s.id == self.active_session_id)
            .or_else(|| self.sessions.first())
    }

    pub fn current_session_mut(&mut self) -> Option<&mut SessionSummaryDto> {
        let id = self.active_session_id.clone();
        if let Some(pos) = self.sessions.iter().position(|s| s.id == id) {
            self.sessions.get_mut(pos)
        } else {
            self.sessions.first_mut()
        }
    }

    pub fn select_session(&mut self, id: &str) {
        self.active_session_id = id.to_string();
        self.board_open = false;
        self.sandbox_details_open = false;
    }

    pub fn grouped_sessions(&self) -> Vec<(SessionUiStatus, Vec<&SessionSummaryDto>)> {
        let order = [
            SessionUiStatus::Blocked,
            SessionUiStatus::Working,
            SessionUiStatus::Done,
            SessionUiStatus::Idle,
        ];
        order
            .into_iter()
            .filter_map(|status| {
                let items: Vec<&SessionSummaryDto> = self
                    .sessions
                    .iter()
                    .filter(|s| s.status == status)
                    .collect();
                if items.is_empty() {
                    None
                } else {
                    Some((status, items))
                }
            })
            .collect()
    }

    pub fn toggle_theme(&mut self) {
        let next = match self.theme.mode {
            ThemeMode::SaintDark => ThemeMode::SaintLight,
            ThemeMode::SaintLight => ThemeMode::SaintDark,
            other => other,
        };
        self.theme = SaintTheme::for_mode(next);
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_user_toggled = true;
    }

    pub fn toggle_diff_drawer(&mut self) {
        if !self.diff_drawer_open {
            self.last_focus_before_overlay = FocusTarget::DiffButton;
        }
        self.diff_drawer_open = !self.diff_drawer_open;
    }

    pub fn toggle_cli_drawer(&mut self) {
        self.cli_drawer_open = !self.cli_drawer_open;
    }

    pub fn toggle_board(&mut self) {
        self.board_open = !self.board_open;
    }

    pub fn open_model_popover(&mut self) {
        self.last_focus_before_overlay = FocusTarget::ModelButton;
        self.cache_popover_open = false;
        self.access_popover_open = false;
        self.model_popover_open = !self.model_popover_open;
    }

    pub fn open_cache_popover(&mut self) {
        self.last_focus_before_overlay = FocusTarget::CacheButton;
        self.model_popover_open = false;
        self.access_popover_open = false;
        self.cache_popover_open = !self.cache_popover_open;
    }

    pub fn open_access_popover(&mut self) {
        self.last_focus_before_overlay = FocusTarget::AccessPill;
        self.model_popover_open = false;
        self.cache_popover_open = false;
        self.access_popover_open = !self.access_popover_open;
    }

    pub fn open_btw(&mut self) {
        self.last_focus_before_overlay = FocusTarget::Composer;
        self.btw_open = true;
    }

    pub fn open_tree_modal(&mut self) {
        self.last_focus_before_overlay = FocusTarget::Composer;
        self.tree_modal_open = true;
    }

    pub fn toggle_latest_worked_for(&mut self) {
        if let Some(sess) = self.current_session_mut()
            && let Some(TranscriptItemDto::Activity { expanded, .. }) = sess
                .messages
                .iter_mut()
                .rev()
                .find(|m| matches!(m, TranscriptItemDto::Activity { .. }))
        {
            *expanded = !*expanded;
        }
    }

    pub fn set_runtime_mode(&mut self, mode: WireRuntimeMode) {
        if let Some(sess) = self.current_session_mut() {
            sess.runtime_mode = mode;
        }
        self.access_popover_open = false;
        self.focus = FocusTarget::AccessPill;
        // Auto-resolve pending workspace shell approval when switching to Auto or Bypass
        let should_auto = self.current_session().is_some_and(|s| {
            s.status == SessionUiStatus::Blocked
                && s.decision.as_ref().is_some_and(|d| d.kind == "shell")
                && matches!(mode, WireRuntimeMode::Auto | WireRuntimeMode::FullAccess)
        });
        if should_auto {
            self.resolve_approval_choice(if mode == WireRuntimeMode::FullAccess {
                2
            } else {
                1
            });
        }
    }

    pub fn toggle_interaction_mode(&mut self) {
        if let Some(sess) = self.current_session_mut() {
            sess.interaction_mode = match sess.interaction_mode {
                WireInteractionMode::Build => WireInteractionMode::Plan,
                WireInteractionMode::Plan => WireInteractionMode::Build,
            };
        }
    }

    /// Resolves approval card shortcuts `1` (once), `2` (always), `3` (decline), `4` (cancel turn -> UserDeclined).
    pub fn resolve_approval_choice(&mut self, choice: u8) -> bool {
        if !(1..=4).contains(&choice) {
            return false;
        }
        let Some(sess_idx) = self
            .sessions
            .iter()
            .position(|s| s.id == self.active_session_id)
            .or(if self.sessions.is_empty() {
                None
            } else {
                Some(0)
            })
        else {
            return false;
        };
        let Some(dec) = self.sessions[sess_idx].decision.clone() else {
            return false;
        };

        self.sessions[sess_idx].decision = None;
        self.sandbox_details_open = false;

        if dec.kind == "ask" {
            let idx = (choice as usize).saturating_sub(1);
            if let Some((label, _)) = dec.options.get(idx) {
                let sess = &mut self.sessions[sess_idx];
                sess.status = SessionUiStatus::Done;
                sess.subtitle = "done".to_string();
                sess.messages.push(TranscriptItemDto::User {
                    text: label.clone(),
                });
                sess.messages.push(TranscriptItemDto::Assistant {
                    text: format!("Applied: {label}."),
                    meta: Some("user_input resolved".to_string()),
                });
                return true;
            }
            return false;
        }

        let sess = &mut self.sessions[sess_idx];
        let sess_req_id = dec.request_id.clone();
        match choice {
            1 | 2 => {
                sess.status = if self.live_connected {
                    SessionUiStatus::Working
                } else {
                    SessionUiStatus::Done
                };
                sess.subtitle = if self.live_connected {
                    "working…".to_string()
                } else {
                    "done".to_string()
                };
                let note = if choice == 2 {
                    "Approved for session (pattern saved)."
                } else {
                    "Approved once and executed inside Landlock WorkspaceWrite."
                };
                sess.messages.push(TranscriptItemDto::Assistant {
                    text: note.to_string(),
                    meta: Some("sandbox=WorkspaceWrite".to_string()),
                });
                self.last_user_declined_emitted = false;
            }
            3 => {
                sess.status = SessionUiStatus::Done;
                sess.subtitle = "adjusted".to_string();
                sess.messages.push(TranscriptItemDto::Assistant {
                    text: "Command declined with feedback. Keeping patch staged.".to_string(),
                    meta: Some("RecoverableError::CorrectedError".to_string()),
                });
                self.last_user_declined_emitted = false;
            }
            4 => {
                sess.status = SessionUiStatus::Done;
                sess.subtitle = "stopped".to_string();
                sess.messages.push(TranscriptItemDto::Assistant {
                    text: "Turn stopped via ToolOutcome::UserDeclined. No command was executed."
                        .to_string(),
                    meta: Some("Law #4 · UserDeclined".to_string()),
                });
                self.last_user_declined_emitted = true;
            }
            _ => {}
        }
        if self.live_connected && !sess_req_id.is_empty() {
            self.pending_actions.push(DaemonAction::ReplyPermission {
                request_id: sess_req_id,
                choice,
            });
        }
        true
    }

    pub fn set_composer_input(&mut self, text: String) {
        self.composer_input = text;
        self.refresh_inline_palette();
    }

    pub fn refresh_inline_palette(&mut self) {
        let Some(first) = self.composer_input.chars().next() else {
            self.inline_palette_open = false;
            self.inline_palette_trigger = None;
            self.inline_palette_items.clear();
            return;
        };
        if !matches!(first, '/' | '@' | '$') {
            self.inline_palette_open = false;
            self.inline_palette_trigger = None;
            self.inline_palette_items.clear();
            return;
        }
        let query = self.composer_input[first.len_utf8()..].to_ascii_lowercase();
        let all: Vec<PaletteOption> = match first {
            '/' => SLASH_COMMANDS
                .iter()
                .map(|(k, d)| PaletteOption {
                    key: (*k).to_string(),
                    description: (*d).to_string(),
                    insert_text: None,
                })
                .collect(),
            '@' => vec![
                PaletteOption {
                    key: "@store/writer.rs".into(),
                    description: "Single SQLite WAL writer thread".into(),
                    insert_text: Some("@crates/saint-harness-core/src/store/writer.rs ".into()),
                },
                PaletteOption {
                    key: "@edit/hashline.rs".into(),
                    description: "FNV-1a 2-char anchor bottom-up apply".into(),
                    insert_text: Some("@crates/saint-harness-edit/src/hashline.rs ".into()),
                },
                PaletteOption {
                    key: "@ai/catalog.rs".into(),
                    description: "Dynamic ModelResolver (0 hardcode)".into(),
                    insert_text: Some("@crates/saint-harness-ai/src/catalog.rs ".into()),
                },
                PaletteOption {
                    key: "@shell/sandbox_linux.rs".into(),
                    description: "Async-signal-safe Landlock V3".into(),
                    insert_text: Some("@crates/saint-harness-shell/src/sandbox_linux.rs ".into()),
                },
            ],
            '$' => vec![
                PaletteOption {
                    key: "$rust-perf-audit".into(),
                    description: "Project skill · .saint-harness/skills".into(),
                    insert_text: Some("$rust-perf-audit ".into()),
                },
                PaletteOption {
                    key: "$saint-bridge".into(),
                    description: "User skill · SHA adapter conformance".into(),
                    insert_text: Some("$saint-bridge ".into()),
                },
                PaletteOption {
                    key: "$mcp:saint".into(),
                    description: "Direct gateway tools (auto-approved)".into(),
                    insert_text: Some("$mcp:saint ".into()),
                },
            ],
            _ => Vec::new(),
        };

        self.inline_palette_items = all
            .into_iter()
            .filter(|opt| {
                let hay = format!("{} {}", opt.key, opt.description).to_ascii_lowercase();
                hay.contains(&query)
            })
            .collect();
        self.inline_palette_idx = 0;
        self.inline_palette_trigger = Some(first);
        self.inline_palette_open = true;
    }

    pub fn palette_move(&mut self, delta: isize) {
        let len = self.inline_palette_items.len();
        if !self.inline_palette_open || len == 0 {
            return;
        }
        let next = ((self.inline_palette_idx as isize) + delta).rem_euclid(len as isize);
        self.inline_palette_idx = next as usize;
    }

    pub fn submit_composer(&mut self) {
        if self.inline_palette_open
            && let Some(chosen) = self
                .inline_palette_items
                .get(self.inline_palette_idx)
                .cloned()
        {
            self.inline_palette_open = false;
            if let Some(insert) = chosen.insert_text {
                self.composer_input = insert;
                return;
            }
            self.composer_input.clear();
            self.execute_slash_command(&chosen.key);
            return;
        }

        let text = self.composer_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.inline_palette_open = false;
        self.composer_input.clear();

        if text.starts_with('/') {
            let cmd = text.split_whitespace().next().unwrap_or("");
            if self.execute_slash_command(cmd) {
                return;
            }
        }

        let is_working = self
            .current_session()
            .is_some_and(|s| s.status == SessionUiStatus::Working);
        if is_working {
            self.steer_banner = Some(format!(
                "Steer queued: \"{text}\". Sent after the current tool finishes."
            ));
            if self.live_connected {
                let sid = self.active_session_id.clone();
                self.pending_actions.push(DaemonAction::SendPrompt {
                    session_id: sid,
                    text,
                    delivery: "steer".to_string(),
                });
            }
            return;
        }

        let live = self.live_connected;
        let sid = self.active_session_id.clone();
        if let Some(sess) = self.current_session_mut() {
            if sess.status == SessionUiStatus::Idle {
                sess.name = text
                    .to_ascii_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .take(22)
                    .collect();
            }
            sess.messages
                .push(TranscriptItemDto::User { text: text.clone() });
            if live {
                sess.status = SessionUiStatus::Working;
                sess.subtitle = "working…".to_string();
            } else {
                sess.status = SessionUiStatus::Done;
                sess.subtitle = "done".to_string();
                sess.messages.push(TranscriptItemDto::Activity {
                    summary: "Worked for 4.2s · 2 tools".to_string(),
                    working: false,
                    expanded: false,
                    tools: vec![
                        ToolTraceDto {
                            verb: "repo_map".into(),
                            arg: "1,500 tok".into(),
                            note: "12 symbols ranked (9ms)".into(),
                            opens_diff: false,
                        },
                        ToolTraceDto {
                            verb: "edit".into(),
                            arg: "store/writer.rs".into(),
                            note: "+14 -2 · hashline ✓".into(),
                            opens_diff: true,
                        },
                    ],
                });
                sess.messages.push(TranscriptItemDto::Assistant {
                    text: format!(
                        "Done: {text}. Wrote the edit through FileWriteCoordinator; tree-sitter and LSP reported no errors."
                    ),
                    meta: Some("18.4K tok · 89% cached · $0.004".to_string()),
                });
            }
        }
        if live {
            self.pending_actions.push(DaemonAction::SendPrompt {
                session_id: sid,
                text,
                delivery: "queue".to_string(),
            });
        }
    }

    pub fn drain_actions(&mut self) -> Vec<DaemonAction> {
        std::mem::take(&mut self.pending_actions)
    }

    /// Applies a real-time SSE wire event streamed from `saint-harness-server` (`/events`).
    pub fn apply_sse_event(&mut self, wire: &SseWireEvent) {
        let sid = wire.session_id.as_str();
        let pos = self.sessions.iter().position(|s| s.id == sid);

        match wire.event_type.as_str() {
            "session.created" => {
                if pos.is_none() {
                    let title = wire.payload["title"]
                        .as_str()
                        .unwrap_or("new session")
                        .to_string();
                    let profile = wire.payload["model_profile"]
                        .as_str()
                        .unwrap_or("qwen-3.8-27b")
                        .to_string();
                    self.sessions.push(SessionSummaryDto {
                        id: sid.to_string(),
                        name: title,
                        status: SessionUiStatus::Idle,
                        model_profile: profile.clone(),
                        model_label: profile,
                        model_key: "qwen".to_string(),
                        ctx_label: "0 / 110K".to_string(),
                        diff: None,
                        subtitle: "main".to_string(),
                        cache_warm: false,
                        cache_label: "Cold · 0%".to_string(),
                        cache_ttl_secs: 0,
                        cache_ttl_max_secs: 300,
                        cache_hit_pct: 0.0,
                        prune_epoch: 0,
                        queued_masks: 0,
                        cost_usd: 0.0,
                        runtime_mode: WireRuntimeMode::ApprovalRequired,
                        interaction_mode: WireInteractionMode::Build,
                        decision: None,
                        messages: Vec::new(),
                    });
                }
            }
            "step.started" => {
                if let Some(i) = pos {
                    self.sessions[i].status = SessionUiStatus::Working;
                    self.sessions[i].subtitle = "working…".to_string();
                }
            }
            "text.delta" => {
                if let Some(i) = pos
                    && let Some(delta) = wire.payload["delta"].as_str()
                {
                    let msgs = &mut self.sessions[i].messages;
                    if let Some(TranscriptItemDto::Assistant { text, .. }) = msgs.last_mut() {
                        text.push_str(delta);
                    } else {
                        msgs.push(TranscriptItemDto::Assistant {
                            text: delta.to_string(),
                            meta: None,
                        });
                    }
                }
            }
            "tool.progress" => {
                if let Some(i) = pos {
                    let name = wire.payload["tool_name"]
                        .as_str()
                        .unwrap_or("tool")
                        .to_string();
                    let status = wire.payload["status"]
                        .as_str()
                        .unwrap_or("running")
                        .to_string();
                    let is_running = status == "running" || status == "started";
                    let msgs = &mut self.sessions[i].messages;
                    if let Some(TranscriptItemDto::Activity {
                        tools,
                        working,
                        summary,
                        ..
                    }) = msgs
                        .iter_mut()
                        .rev()
                        .find(|m| matches!(m, TranscriptItemDto::Activity { .. }))
                    {
                        if !is_running
                            && let Some(last) = tools.iter_mut().rev().find(|t| t.verb == name)
                        {
                            last.note = status;
                        } else {
                            tools.push(ToolTraceDto {
                                verb: name,
                                arg: String::new(),
                                note: status,
                                opens_diff: false,
                            });
                        }
                        *working = is_running;
                        *summary = if is_running {
                            format!("Working · {} tools", tools.len())
                        } else {
                            format!("Worked · {} tools", tools.len())
                        };
                    } else {
                        msgs.push(TranscriptItemDto::Activity {
                            summary: "Working · 1 tool".to_string(),
                            working: is_running,
                            expanded: false,
                            tools: vec![ToolTraceDto {
                                verb: name,
                                arg: String::new(),
                                note: status,
                                opens_diff: false,
                            }],
                        });
                    }
                }
            }
            "permission.requested" => {
                if let Some(i) = pos {
                    let req = &wire.payload["request"];
                    let request_id = req["request_id"].as_str().unwrap_or("").to_string();
                    let title = req["title"]
                        .as_str()
                        .unwrap_or("Approve this command?")
                        .to_string();
                    let resource = req["resource"].as_str().unwrap_or("").to_string();
                    let sandbox = req["sandbox_summary"]
                        .as_str()
                        .unwrap_or("WorkspaceWrite")
                        .to_string();
                    self.sessions[i].status = SessionUiStatus::Blocked;
                    self.sessions[i].subtitle = "1 approval".to_string();
                    self.sessions[i].decision = Some(DecisionCardDto {
                        request_id,
                        kind: "shell".to_string(),
                        title,
                        command: Some(resource),
                        sandbox: Some(sandbox),
                        sandbox_info: None,
                        pattern: req["suggested_pattern"].as_str().map(ToString::to_string),
                        options: Vec::new(),
                    });
                }
            }
            "turn.diff.updated" => {
                if let Some(i) = pos {
                    let added = wire.payload["lines_added"].as_u64().unwrap_or(0) as usize;
                    let removed = wire.payload["lines_removed"].as_u64().unwrap_or(0) as usize;
                    self.sessions[i].diff = Some(DiffStatDto { added, removed });
                }
            }
            "step.ended" => {
                if let Some(i) = pos {
                    self.sessions[i].status = SessionUiStatus::Done;
                    self.sessions[i].subtitle = "done".to_string();
                    let usage = &wire.payload["usage"];
                    let inp = usage["input_tokens"].as_u64().unwrap_or(0);
                    let cached = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                    let cost = wire.payload["cost_usd"].as_f64().unwrap_or(0.0);
                    self.sessions[i].cost_usd += cost;
                    if inp > 0 {
                        let pct = (cached as f64) * 100.0 / (inp as f64);
                        self.sessions[i].cache_hit_pct = pct;
                        self.sessions[i].cache_warm = cached > 0;
                        self.sessions[i].cache_label =
                            format!("{} · {:.0}%", if cached > 0 { "Warm" } else { "Cold" }, pct);
                    }
                }
            }
            "step.failed" => {
                if let Some(i) = pos {
                    let err = wire.payload["error"]
                        .as_str()
                        .unwrap_or("step failed")
                        .to_string();
                    self.sessions[i].status = SessionUiStatus::Done;
                    self.sessions[i].subtitle = "error".to_string();
                    self.sessions[i]
                        .messages
                        .push(TranscriptItemDto::Assistant {
                            text: format!("Error: {err}"),
                            meta: Some("step failed".to_string()),
                        });
                }
            }
            _ => {}
        }
    }

    pub fn execute_slash_command(&mut self, cmd: &str) -> bool {
        match cmd {
            "/plan" => {
                self.toggle_interaction_mode();
                true
            }
            "/auto" => {
                self.set_runtime_mode(WireRuntimeMode::Auto);
                true
            }
            "/bypass" => {
                self.set_runtime_mode(WireRuntimeMode::FullAccess);
                true
            }
            "/supervised" => {
                self.set_runtime_mode(WireRuntimeMode::ApprovalRequired);
                true
            }
            "/diff" => {
                self.toggle_diff_drawer();
                true
            }
            "/btw" => {
                self.open_btw();
                true
            }
            "/tree" => {
                self.open_tree_modal();
                true
            }
            "/board" => {
                self.board_open = true;
                true
            }
            "/model" => {
                self.open_model_popover();
                true
            }
            "/compact" => {
                if let Some(sess) = self.current_session_mut()
                    && !sess.messages.is_empty()
                {
                    sess.messages.push(TranscriptItemDto::Handoff {
                        text: "Tier 1 shake: 3 stale tool outputs masked, no LLM call".to_string(),
                    });
                }
                true
            }
            "/cli" => {
                self.toggle_cli_drawer();
                true
            }
            "/offline" => {
                self.daemon_state = DaemonConnectionState::Error;
                self.sidebar_open = true;
                true
            }
            "/auth" | "/login" => {
                if let Some(sess) = self.current_session_mut() {
                    sess.messages.push(TranscriptItemDto::Handoff {
                        text: "Local API keys live in ~/.saint-harness/settings.json (chmod 0600). Run `saint-harness auth login` or `saint-harness auth status`.".to_string(),
                    });
                }
                true
            }
            _ => false,
        }
    }

    pub fn toggle_provider_expand(&mut self, provider_key: &str) {
        if self.expanded_providers.contains(provider_key) {
            self.expanded_providers.remove(provider_key);
        } else {
            self.expanded_providers.insert(provider_key.to_string());
            if self.daemon_state == DaemonConnectionState::Error {
                self.provider_catalog_state
                    .insert(provider_key.to_string(), CatalogLoadState::Error);
            } else {
                let cur = self
                    .provider_catalog_state
                    .get(provider_key)
                    .copied()
                    .unwrap_or(CatalogLoadState::Idle);
                if cur != CatalogLoadState::Loaded {
                    self.provider_catalog_state
                        .insert(provider_key.to_string(), CatalogLoadState::Loading);
                }
            }
        }
    }

    pub fn finish_provider_catalog_load(&mut self, provider_key: &str, ok: bool) {
        self.provider_catalog_state.insert(
            provider_key.to_string(),
            if ok {
                CatalogLoadState::Loaded
            } else {
                CatalogLoadState::Error
            },
        );
    }

    pub fn select_model_key(&mut self, key: &str) -> bool {
        if let Some(leaf) = self.model_leaves.iter().find(|l| l.key == key).cloned() {
            if let Some(sess) = self.current_session_mut() {
                sess.model_label = leaf.name;
                sess.model_key = leaf.key;
                if !sess.messages.is_empty() {
                    sess.messages.push(TranscriptItemDto::Handoff {
                        text: leaf.handoff_note,
                    });
                }
            }
            self.model_popover_open = false;
            self.focus = FocusTarget::ModelButton;
            return true;
        }
        for prov in &self.model_providers {
            if let Some(child) = prov.children.iter().find(|c| c.key == key).cloned() {
                let note = format!(
                    "Handoff to {} ({}) · {}",
                    child.name, child.selector, prov.handoff_suffix
                );
                if let Some(sess) = self.current_session_mut() {
                    sess.model_label = child.name;
                    sess.model_key = child.key;
                    if !sess.messages.is_empty() {
                        sess.messages
                            .push(TranscriptItemDto::Handoff { text: note });
                    }
                }
                self.model_popover_open = false;
                self.focus = FocusTarget::ModelButton;
                return true;
            }
        }
        false
    }

    /// Closes ONLY the top-most open layer and restores focus to its trigger (contract invariant).
    pub fn handle_escape(&mut self, cols: u16) -> bool {
        if self.tree_modal_open {
            self.tree_modal_open = false;
            self.focus = self.last_focus_before_overlay;
            return true;
        }
        if self.inline_palette_open {
            self.inline_palette_open = false;
            self.focus = FocusTarget::Composer;
            return true;
        }
        if self.access_popover_open {
            self.access_popover_open = false;
            self.focus = FocusTarget::AccessPill;
            return true;
        }
        if self.model_popover_open {
            self.model_popover_open = false;
            self.focus = FocusTarget::ModelButton;
            return true;
        }
        if self.cache_popover_open {
            self.cache_popover_open = false;
            self.focus = FocusTarget::CacheButton;
            return true;
        }
        if self.btw_open {
            self.btw_open = false;
            self.focus = FocusTarget::Composer;
            return true;
        }
        if cols < 120 && self.diff_drawer_open {
            self.diff_drawer_open = false;
            self.focus = FocusTarget::DiffButton;
            return true;
        }
        if cols < 120 && self.sidebar_open {
            self.sidebar_open = false;
            self.focus = FocusTarget::SidebarButton;
            return true;
        }
        if self.board_open {
            self.board_open = false;
            self.focus = FocusTarget::Composer;
            return true;
        }
        false
    }
}

fn default_model_leaves() -> Vec<ModelLeafEntry> {
    vec![
        ModelLeafEntry {
            key: "qwen".into(),
            name: "Qwen 3.8".into(),
            label: "Qwen 3.8 27B".into(),
            selector: "exact:qwen3.8-27b".into(),
            tag: "hashline · local".into(),
            handoff_note: "Switched to Qwen 3.8 27B · SGLang Radix prefix warm · Hashline".into(),
        },
        ModelLeafEntry {
            key: "qwen4".into(),
            name: "Qwen 4".into(),
            label: "Qwen 4".into(),
            selector: "latest_family:qwen4".into(),
            tag: "latest_family".into(),
            handoff_note: "Switched to Qwen 4 (latest_family:qwen4) · DashScope cache".into(),
        },
    ]
}

fn default_model_providers() -> Vec<ModelProviderNode> {
    vec![
        ModelProviderNode {
            key: "claude".into(),
            label: "Claude".into(),
            catalog: "Anthropic".into(),
            tag: "Inline4 cache".into(),
            handoff_suffix: "Inline4 breakpoints · foreign thinking stripped".into(),
            footnote: None,
            children: vec![
                ModelLeafEntry {
                    key: "claude".into(),
                    name: "Claude Flagship".into(),
                    label: "Auto".into(),
                    selector: "latest_flagship".into(),
                    tag: "latest_flagship".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "claude-fable-5-2".into(),
                    name: "Claude Fable 5.2".into(),
                    label: "Fable 5.2".into(),
                    selector: "claude-fable-5-2".into(),
                    tag: "claude-fable-5-2".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "claude-opus-5-5".into(),
                    name: "Claude Opus 5.5".into(),
                    label: "Opus 5.5".into(),
                    selector: "claude-opus-5-5".into(),
                    tag: "claude-opus-5-5".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "claude-sonnet-5-5".into(),
                    name: "Claude Sonnet 5.5".into(),
                    label: "Sonnet 5.5".into(),
                    selector: "claude-sonnet-5-5".into(),
                    tag: "claude-sonnet-5-5".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "claude-haiku-5-5".into(),
                    name: "Claude Haiku 5.5".into(),
                    label: "Haiku 5.5".into(),
                    selector: "claude-haiku-5-5".into(),
                    tag: "claude-haiku-5-5".into(),
                    handoff_note: String::new(),
                },
            ],
        },
        ModelProviderNode {
            key: "gemini".into(),
            label: "Gemini".into(),
            catalog: "Google".into(),
            tag: "implicit cache".into(),
            handoff_suffix: "skip_thought_signature_validator active".into(),
            footnote: Some("Exact Gemini IDs resolve from the live catalog.".into()),
            children: vec![
                ModelLeafEntry {
                    key: "gemini".into(),
                    name: "Gemini Flagship".into(),
                    label: "Auto".into(),
                    selector: "latest_flagship".into(),
                    tag: "latest_flagship".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "gemini-pro".into(),
                    name: "Gemini Pro".into(),
                    label: "Pro".into(),
                    selector: "latest_family:gemini-pro".into(),
                    tag: "latest_family:gemini-pro".into(),
                    handoff_note: String::new(),
                },
                ModelLeafEntry {
                    key: "gemini-flash".into(),
                    name: "Gemini Flash".into(),
                    label: "Flash".into(),
                    selector: "latest_family:gemini-flash".into(),
                    tag: "latest_family:gemini-flash".into(),
                    handoff_note: String::new(),
                },
            ],
        },
    ]
}

fn sample_diff_files() -> Vec<(String, Vec<DiffLineEntry>)> {
    vec![
        (
            "store/writer.rs".into(),
            vec![
                DiffLineEntry { hashline: "35#PT│".into(), line_num: "35".into(), sign: ' ', code: "fn apply_batch(&mut self, cmd: WriteCmd) -> Result<Receipt> {".into() },
                DiffLineEntry { hashline: "36#BX│".into(), line_num: "36".into(), sign: ' ', code: "    self.conn.busy_timeout(Duration::from_millis(5_000))?;".into() },
                DiffLineEntry { hashline: "37#VR│".into(), line_num: "37".into(), sign: '-', code: "    let tx = self.conn.transaction()?;".into() },
                DiffLineEntry { hashline: "37#KB│".into(), line_num: "37".into(), sign: '+', code: "    let tx = self.conn".into() },
                DiffLineEntry { hashline: "38#JN│".into(), line_num: "38".into(), sign: '+', code: "        .transaction_with_behavior(TransactionBehavior::Immediate)?;".into() },
                DiffLineEntry { hashline: "39#SW│".into(), line_num: "39".into(), sign: ' ', code: "    let seq = bump_sequence(&tx, &cmd.aggregate_id)?;".into() },
            ],
        ),
        (
            "store/projector.rs".into(),
            vec![
                DiffLineEntry { hashline: "82#ZT│".into(), line_num: "82".into(), sign: ' ', code: "SessionEvent::FileReadRecorded { path, blake3, epoch } => {".into() },
                DiffLineEntry { hashline: "83#JB│".into(), line_num: "83".into(), sign: '+', code: "    tx.execute(\"INSERT OR REPLACE INTO session_file_read_state ...\", ...)?;".into() },
            ],
        ),
    ]
}

fn sample_sessions() -> Vec<SessionSummaryDto> {
    vec![
        SessionSummaryDto {
            id: "blocked-shell".into(),
            name: "feat/mcp-direct".into(),
            status: SessionUiStatus::Blocked,
            model_profile: "qwen-3.8-27b".into(),
            model_label: "Qwen 3.8".into(),
            model_key: "qwen".into(),
            ctx_label: "39K / 110K".into(),
            diff: Some(DiffStatDto { added: 18, removed: 4 }),
            subtitle: "1 approval".into(),
            cache_warm: true,
            cache_label: "Warm 3m48s · 86%".into(),
            cache_ttl_secs: 228,
            cache_ttl_max_secs: 300,
            cache_hit_pct: 86.0,
            prune_epoch: 2,
            queued_masks: 2,
            cost_usd: 0.009,
            runtime_mode: WireRuntimeMode::ApprovalRequired,
            interaction_mode: WireInteractionMode::Build,
            decision: Some(DecisionCardDto {
                request_id: "perm_sample_1".into(),
                kind: "shell".into(),
                title: "Approve this command?".into(),
                command: Some("cargo nextest run -p saint-harness-core".into()),
                sandbox: Some("WorkspaceWrite".into()),
                sandbox_info: Some("RW: /SaintHarness, /tmp/sh · RO: /usr, ~/.cargo · Blocked: ~/.ssh, *_API_KEY scrubbed".into()),
                pattern: Some("cargo nextest *".into()),
                options: vec![],
            }),
            messages: vec![
                TranscriptItemDto::User {
                    text: "Wire McpExposure::Direct for the Saint Gateway server so mcp__saint__* tools never hide inside CodeMode.".into(),
                },
                TranscriptItemDto::Activity {
                    summary: "Worked for 6.1s · 3 tools".into(),
                    working: false,
                    expanded: false,
                    tools: vec![
                        ToolTraceDto { verb: "read".into(), arg: "mcp/router.rs".into(), note: "outline 140L → 26L".into(), opens_diff: false },
                        ToolTraceDto { verb: "edit".into(), arg: "mcp/router.rs".into(), note: "+18 -4 · hashline 1st-try ✓".into(), opens_diff: true },
                        ToolTraceDto { verb: "shell".into(), arg: "cargo nextest run -p saint-harness-core".into(), note: "awaiting approval".into(), opens_diff: false },
                    ],
                },
            ],
        },
        SessionSummaryDto {
            id: "blocked-ask".into(),
            name: "design/snapshot-api".into(),
            status: SessionUiStatus::Blocked,
            model_profile: "claude-flagship".into(),
            model_label: "Claude Flagship".into(),
            model_key: "claude".into(),
            ctx_label: "28K / 200K".into(),
            diff: None,
            subtitle: "question".into(),
            cache_warm: true,
            cache_label: "Warm 4m30s · 92%".into(),
            cache_ttl_secs: 270,
            cache_ttl_max_secs: 300,
            cache_hit_pct: 92.0,
            prune_epoch: 1,
            queued_masks: 1,
            cost_usd: 0.018,
            runtime_mode: WireRuntimeMode::ApprovalRequired,
            interaction_mode: WireInteractionMode::Build,
            decision: Some(DecisionCardDto {
                request_id: "ask_sample_1".into(),
                kind: "ask".into(),
                title: "Which Git snapshot mode when managed by Saint Hyper Agent?".into(),
                command: None,
                sandbox: None,
                sandbox_info: None,
                pattern: None,
                options: vec![
                    ("Delegate to Saint Checkpointing (Recommended)".into(), "Sets disable_internal_git_snapshot=true to avoid double I/O".into()),
                    ("Keep internal Shadow Git Snapshot active".into(), "Independent 3-phase stage/clear/commit inside SaintHarness".into()),
                    ("Pure Rust gix read-only diff only".into(), "Live diff patches without writing shadow commits".into()),
                ],
            }),
            messages: vec![
                TranscriptItemDto::User {
                    text: "Align SaintHarness shadow snapshot lifecycle with Saint Hyper Agent's workspace checkpointing.".into(),
                },
            ],
        },
        SessionSummaryDto {
            id: "working-live".into(),
            name: "refactor/hashline".into(),
            status: SessionUiStatus::Working,
            model_profile: "qwen-3.8-27b".into(),
            model_label: "Qwen 3.8".into(),
            model_key: "qwen".into(),
            ctx_label: "52K / 110K".into(),
            diff: Some(DiffStatDto { added: 31, removed: 12 }),
            subtitle: "running tests…".into(),
            cache_warm: true,
            cache_label: "Warm 4m45s · 90%".into(),
            cache_ttl_secs: 285,
            cache_ttl_max_secs: 300,
            cache_hit_pct: 90.0,
            prune_epoch: 2,
            queued_masks: 4,
            cost_usd: 0.011,
            runtime_mode: WireRuntimeMode::Auto,
            interaction_mode: WireInteractionMode::Build,
            decision: None,
            messages: vec![
                TranscriptItemDto::User {
                    text: "Add ±4 line self-healing context snippet to StaleAnchorError.".into(),
                },
                TranscriptItemDto::Activity {
                    summary: "Working… · 4 tools (running cargo nextest)".into(),
                    working: true,
                    expanded: true,
                    tools: vec![
                        ToolTraceDto { verb: "read".into(), arg: "edit/src/hashline.rs".into(), note: "[Unchanged · delta 0B]".into(), opens_diff: false },
                        ToolTraceDto { verb: "edit".into(), arg: "edit/src/hashline.rs".into(), note: "+31 -12 · imara-diff ✓".into(), opens_diff: true },
                    ],
                },
            ],
        },
        SessionSummaryDto {
            id: "done-wal".into(),
            name: "fix/wal-lock".into(),
            status: SessionUiStatus::Done,
            model_profile: "qwen-3.8-27b".into(),
            model_label: "Qwen 3.8".into(),
            model_key: "qwen".into(),
            ctx_label: "48K / 110K".into(),
            diff: Some(DiffStatDto { added: 24, removed: 6 }),
            subtitle: "done".into(),
            cache_warm: true,
            cache_label: "Warm 4m12s · 88%".into(),
            cache_ttl_secs: 252,
            cache_ttl_max_secs: 300,
            cache_hit_pct: 88.0,
            prune_epoch: 2,
            queued_masks: 3,
            cost_usd: 0.014,
            runtime_mode: WireRuntimeMode::ApprovalRequired,
            interaction_mode: WireInteractionMode::Build,
            decision: None,
            messages: vec![
                TranscriptItemDto::User {
                    text: "Fix SQLITE_BUSY under 32 concurrent Tokio writers in store/writer.rs and verify contiguous sequences 1..1000.".into(),
                },
                TranscriptItemDto::Activity {
                    summary: "Worked for 9.4s · 5 tools".into(),
                    working: false,
                    expanded: false,
                    tools: vec![
                        ToolTraceDto { verb: "repo_map".into(), arg: "14 symbols".into(), note: "1,500 tok budget · 11ms".into(), opens_diff: false },
                        ToolTraceDto { verb: "read".into(), arg: "store/writer.rs".into(), note: "outline 184L → 32L (83% saved)".into(), opens_diff: false },
                        ToolTraceDto { verb: "read".into(), arg: "store/bus.rs".into(), note: "[Unchanged · delta 0B]".into(), opens_diff: false },
                        ToolTraceDto { verb: "edit".into(), arg: "store/writer.rs".into(), note: "+24 -6 · hashline 1st-try ✓".into(), opens_diff: true },
                        ToolTraceDto { verb: "shell".into(), arg: "cargo nextest run -p saint-harness-core".into(), note: "exit 0 · 1.8s · 412L→14L".into(), opens_diff: false },
                    ],
                },
                TranscriptItemDto::Assistant {
                    text: "Switched the dedicated sh-sqlite-writer thread to TransactionBehavior::Immediate with a 5000ms busy timeout, committing event + projector + commit hooks in one atomic transaction before broadcasting SSE.".into(),
                    meta: Some("48.2K tok · 88% cached · ↓74% condensed · $0.014".into()),
                },
            ],
        },
        SessionSummaryDto {
            id: "idle-new".into(),
            name: "new session".into(),
            status: SessionUiStatus::Idle,
            model_profile: "qwen-3.8-27b".into(),
            model_label: "Qwen 3.8".into(),
            model_key: "qwen".into(),
            ctx_label: "0 / 110K".into(),
            diff: None,
            subtitle: "main".into(),
            cache_warm: false,
            cache_label: "Cold · 0%".into(),
            cache_ttl_secs: 0,
            cache_ttl_max_secs: 300,
            cache_hit_pct: 0.0,
            prune_epoch: 0,
            queued_masks: 0,
            cost_usd: 0.0,
            runtime_mode: WireRuntimeMode::ApprovalRequired,
            interaction_mode: WireInteractionMode::Build,
            decision: None,
            messages: vec![],
        },
    ]
}
