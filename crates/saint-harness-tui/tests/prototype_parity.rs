#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use saint_harness_protocol::{SessionUiStatus, TranscriptItemDto, WireRuntimeMode};
use saint_harness_tui::{
    BRAILLE_COMPACT_CROWN_ROWS, BRAILLE_COMPACT_LINES, BRAILLE_STANDARD_CROWN_ROWS,
    BRAILLE_STANDARD_LINES, COMPOSER_FOOTER_ITEMS, CatalogLoadState, CliTab, DaemonConnectionState,
    FocusTarget, LANDING_WAVE_SETTLES_AFTER_MS, SLASH_COMMANDS, SPINNER_FRAMES,
    SPINNER_INTERVAL_MS, SaintTheme, ThemeMode, TuiApp, contrast_ratio, resolve_layout, view,
};
use serde_json::Value;
use std::path::PathBuf;

fn load_contract() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../prototypes/tui-parity-contract.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("valid tui-parity-contract.json")
}

fn render_buffer_text(app: &TuiApp, cols: u16, rows: u16) -> String {
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| view::render(f, app)).unwrap();
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..rows {
        for x in 0..cols {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn render_sidebar_collapsed_text(app: &TuiApp, cols: u16, rows: u16) -> String {
    let spec = resolve_layout(cols, rows);
    let sb_w = spec.sidebar_cols.min(cols / 3);
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| view::render(f, app)).unwrap();
    let buf = terminal.backend().buffer();
    let mut words = Vec::new();
    for y in 1..rows {
        let mut line = String::new();
        for x in 0..sb_w.saturating_sub(1) {
            line.push_str(buf[(x, y)].symbol());
        }
        for w in line.split_whitespace() {
            words.push(w.to_string());
        }
    }
    words.join(" ")
}

fn assert_anti_slop_buffer(text: &str, label: &str) {
    assert!(
        !text.contains('\u{2014}'),
        "{label}: rendered UI contains forbidden em dash (U+2014)"
    );
    assert!(
        !text.contains("●●●") && !text.contains("● ● ●"),
        "{label}: rendered UI contains forbidden fake window traffic lights"
    );
    assert!(
        !text
            .chars()
            .any(|c| ('\u{1F300}'..='\u{1FAFF}').contains(&c)),
        "{label}: rendered UI contains forbidden decorative emoji"
    );
}

#[test]
fn parity_56_theme_rgb_tokens_and_wcag_aa_contrast() {
    let contract = load_contract();
    let min_ratio = contract["theme"]["minTextContrastRatio"].as_f64().unwrap();

    for (theme_name, theme) in [("dark", SaintTheme::dark()), ("light", SaintTheme::light())] {
        let ct = &contract["theme"][theme_name];
        let pairs: [(&str, ratatui::style::Color); 28] = [
            ("surface", theme.surface),
            ("surfaceUnder", theme.surface_under),
            ("sidebarBg", theme.sidebar_bg),
            ("panel", theme.panel),
            ("controlBg", theme.control_bg),
            ("elevatedBg", theme.elevated_bg),
            ("userBubble", theme.user_bubble),
            ("inkPrimary", theme.ink_primary),
            ("inkSecondary", theme.ink_secondary),
            ("inkTertiary", theme.ink_tertiary),
            ("border", theme.border),
            ("borderHeavy", theme.border_heavy),
            ("dividerHairline", theme.divider_hairline),
            ("accent", theme.accent),
            ("textAccent", theme.text_accent),
            ("accentBg", theme.accent_bg),
            ("fullAccess", theme.full_access),
            ("fullAccessInk", theme.full_access_ink),
            ("statusDone", theme.status_done),
            ("statusDoneInk", theme.status_done_ink),
            ("statusBlocked", theme.status_blocked),
            ("statusBlockedInk", theme.status_blocked_ink),
            ("statusWorking", theme.status_working),
            ("statusWorkingInk", theme.status_working_ink),
            ("statusError", theme.status_error),
            ("statusErrorInk", theme.status_error_ink),
            ("diffAddedBg", theme.diff_added_bg),
            ("diffRemovedBg", theme.diff_removed_bg),
        ];

        for (key, color) in pairs {
            let expected: Vec<u8> = ct[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let actual = SaintTheme::token_rgb(color).unwrap();
            assert_eq!(
                actual.as_slice(),
                expected.as_slice(),
                "RGB mismatch on {theme_name}.{key}"
            );
        }

        // Verify WCAG AA >= 4.5:1 for all text inks across all surfaces in both themes.
        let surfaces = [
            theme.surface,
            theme.sidebar_bg,
            theme.panel,
            theme.control_bg,
            theme.elevated_bg,
            theme.user_bubble,
        ];
        let inks = [
            theme.ink_primary,
            theme.ink_secondary,
            theme.ink_tertiary,
            theme.text_accent,
            theme.full_access_ink,
            theme.status_done_ink,
            theme.status_blocked_ink,
            theme.status_working_ink,
            theme.status_error_ink,
        ];
        for s in surfaces {
            let bg = SaintTheme::token_rgb(s).unwrap();
            for i in inks {
                let fg = SaintTheme::token_rgb(i).unwrap();
                let r = contrast_ratio(fg, bg);
                assert!(
                    r >= min_ratio,
                    "WCAG AA contrast failed in {theme_name}: {fg:?} on {bg:?} = {r:.2} < {min_ratio}"
                );
            }
        }
    }
}

#[test]
fn parity_brand_marks_spinner_and_motion_constants() {
    let contract = load_contract();

    let std_lines: Vec<&str> = contract["brandMarks"]["brailleStandard"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(BRAILLE_STANDARD_LINES.as_slice(), std_lines.as_slice());

    let std_crown: Vec<usize> = contract["brandMarks"]["brailleStandard"]["crownRows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(BRAILLE_STANDARD_CROWN_ROWS.as_slice(), std_crown.as_slice());

    let cmp_lines: Vec<&str> = contract["brandMarks"]["brailleCompact"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(BRAILLE_COMPACT_LINES.as_slice(), cmp_lines.as_slice());

    let cmp_crown: Vec<usize> = contract["brandMarks"]["brailleCompact"]["crownRows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(BRAILLE_COMPACT_CROWN_ROWS.as_slice(), cmp_crown.as_slice());

    let spin: Vec<&str> = contract["invariants"]["spinnerFrames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(SPINNER_FRAMES.as_slice(), spin.as_slice());
    assert_eq!(
        SPINNER_INTERVAL_MS,
        contract["invariants"]["spinnerIntervalMs"]
            .as_u64()
            .unwrap()
    );
    assert_eq!(
        LANDING_WAVE_SETTLES_AFTER_MS,
        contract["invariants"]["landingWaveSettlesAfterMs"]
            .as_u64()
            .unwrap()
    );
}

#[test]
fn parity_all_five_responsive_breakpoints_render_cleanly_in_dark_light_and_no_color() {
    let contract = load_contract();
    let bps = contract["responsiveBreakpoints"].as_array().unwrap();
    assert_eq!(bps.len(), 5);

    for bp in bps {
        let name = bp["name"].as_str().unwrap();
        let cols = bp["cols"].as_u64().unwrap() as u16;
        let rows = bp["rows"].as_u64().unwrap() as u16;
        let spec = resolve_layout(cols, rows);

        assert_eq!(spec.breakpoint_name, name);
        assert_eq!(
            spec.sidebar_default_open,
            bp["sidebarDefaultOpen"].as_bool().unwrap()
        );
        assert_eq!(
            spec.diff_drawer_mode.as_contract_name(),
            bp["diffDrawerMode"].as_str().unwrap()
        );
        assert_eq!(
            spec.board_columns,
            bp["boardColumns"].as_u64().unwrap() as usize
        );
        assert_eq!(
            spec.logo_variant.as_contract_name(),
            bp["logoVariant"].as_str().unwrap()
        );
        assert_eq!(
            spec.max_header_glance_items,
            bp["maxHeaderGlanceItems"].as_u64().unwrap() as usize
        );

        for mode in [
            ThemeMode::SaintDark,
            ThemeMode::SaintLight,
            ThemeMode::NoColor,
        ] {
            let mut app = TuiApp::new(mode, cols, rows, false);
            let txt = render_buffer_text(&app, cols, rows);
            assert_anti_slop_buffer(&txt, &format!("{name}/{mode:?}/active"));
            assert!(txt.contains("SaintHarness"));

            // Landing view
            app.select_session("idle-new");
            let landing_txt = render_buffer_text(&app, cols, rows);
            assert_anti_slop_buffer(&landing_txt, &format!("{name}/{mode:?}/landing"));
            assert!(landing_txt.contains("Let's build"));

            // Board view
            app.toggle_board();
            let board_txt = render_buffer_text(&app, cols, rows);
            assert_anti_slop_buffer(&board_txt, &format!("{name}/{mode:?}/board"));
            assert!(board_txt.contains("Agent Board"));
        }
    }
}

#[test]
fn parity_invariants_progressive_disclosure_sidebar_order_footer_and_escape_stack() {
    let contract = load_contract();
    let mut app = TuiApp::new(ThemeMode::SaintDark, 140, 42, false);

    // Progressive disclosure defaults
    assert!(!app.diff_drawer_open);
    assert!(!app.cli_drawer_open);
    assert!(!app.cache_popover_open);
    assert!(!app.sandbox_details_open);
    let done_sess = app.current_session().unwrap();
    let act_expanded = done_sess.messages.iter().find_map(|m| match m {
        TranscriptItemDto::Activity { expanded, .. } => Some(*expanded),
        _ => None,
    });
    assert_eq!(
        act_expanded,
        Some(false),
        "Worked for... must be collapsed by default"
    );

    // Sidebar group order strictly blocked > working > done > idle
    let order: Vec<&str> = app
        .grouped_sessions()
        .into_iter()
        .map(|(st, _)| match st {
            SessionUiStatus::Blocked => "blocked",
            SessionUiStatus::Working => "working",
            SessionUiStatus::Done => "done",
            SessionUiStatus::Idle => "idle",
        })
        .collect();
    let expected_order: Vec<&str> = contract["invariants"]["sidebarGroupOrder"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(order, expected_order);

    // Composer footer items: exact match, NO /btw or /tree buttons in footer
    let contract_footer: Vec<&str> = contract["invariants"]["composerFooterItems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(COMPOSER_FOOTER_ITEMS.as_slice(), contract_footer.as_slice());

    let screen = render_buffer_text(&app, 140, 42);
    let footer_line = screen.lines().rev().nth(1).unwrap_or("");
    assert!(
        !footer_line.contains("/btw"),
        "footer must NOT render /btw button"
    );
    assert!(
        !footer_line.contains("/tree"),
        "footer must NOT render /tree button"
    );

    // Slash commands exact match
    let contract_cmds: Vec<&str> = contract["invariants"]["slashCommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let actual_cmds: Vec<&str> = SLASH_COMMANDS.iter().map(|(k, _)| *k).collect();
    assert_eq!(actual_cmds, contract_cmds);

    // Approval card shortcuts 1-4 & UserDeclined invariant
    app.select_session("blocked-shell");
    let approval_screen = render_buffer_text(&app, 140, 42);
    assert!(approval_screen.contains("Approve this command?"));
    assert!(approval_screen.contains("(1)"));
    assert!(approval_screen.contains("(4)"));
    assert!(app.resolve_approval_choice(4));
    assert!(app.last_user_declined_emitted);

    // Runtime modes: approval-required, auto, full-access
    app.set_runtime_mode(WireRuntimeMode::Auto);
    assert!(render_buffer_text(&app, 140, 42).contains("Auto mode"));
    app.set_runtime_mode(WireRuntimeMode::FullAccess);
    assert!(render_buffer_text(&app, 140, 42).contains("Bypass permissions"));

    // Escape closes top-most layer only and restores focus
    app.open_model_popover();
    app.open_tree_modal();
    assert!(app.model_popover_open && app.tree_modal_open);
    assert!(app.handle_escape(140));
    assert!(!app.tree_modal_open && app.model_popover_open);
    assert!(app.handle_escape(140));
    assert!(!app.model_popover_open);
    assert_eq!(app.focus, FocusTarget::ModelButton);
}

#[test]
fn parity_model_picker_tree_and_required_loading_error_empty_states() {
    let contract = load_contract();
    let mut app = TuiApp::new(ThemeMode::SaintDark, 140, 42, false);

    // Verify model picker tree leaves & providers match contract
    let leaves: Vec<&str> = contract["modelPicker"]["leaves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["selector"].as_str().unwrap())
        .collect();
    let actual_leaves: Vec<&str> = app
        .model_leaves
        .iter()
        .map(|l| l.selector.as_str())
        .collect();
    assert_eq!(actual_leaves, leaves);

    // Open model picker, expand Claude -> Loading -> Loaded -> select Opus 5.5 -> handoff divider written
    app.open_model_popover();
    app.toggle_provider_expand("claude");
    assert_eq!(
        app.provider_catalog_state.get("claude").copied(),
        Some(CatalogLoadState::Loading)
    );
    let loading_txt = render_buffer_text(&app, 140, 42);
    assert!(loading_txt.contains("Loading Claude models from the Anthropic catalog"));

    app.finish_provider_catalog_load("claude", true);
    let loaded_txt = render_buffer_text(&app, 140, 42);
    assert!(loaded_txt.contains("Opus 5.5"));
    assert!(loaded_txt.contains("claude-opus-5-5"));

    assert!(app.select_model_key("claude-opus-5-5"));
    let sess = app.current_session().unwrap();
    assert_eq!(sess.model_label, "Claude Opus 5.5");
    assert!(sess.messages.iter().any(|m| matches!(
        m,
        TranscriptItemDto::Handoff { text } if text.contains("Claude Opus 5.5")
    )));

    // Required UI states: inline palette noMatch
    app.set_composer_input("/doesnotexist".into());
    assert!(render_buffer_text(&app, 140, 42).contains("No match for"));
    app.handle_escape(140);

    // Required UI states: diffDrawer empty on idle session
    app.select_session("idle-new");
    app.toggle_diff_drawer();
    assert!(render_buffer_text(&app, 140, 42).contains("No changes in this session yet"));
    app.toggle_diff_drawer();

    // Required UI states: daemon loading, error, offline CLI status, and offline model catalog
    app.daemon_state = DaemonConnectionState::Loading;
    assert!(
        render_sidebar_collapsed_text(&app, 140, 42).contains("Loading sessions from the daemon")
    );

    app.daemon_state = DaemonConnectionState::Error;
    app.cli_tab = CliTab::Status;
    app.cli_drawer_open = true;
    let offline_sidebar = render_sidebar_collapsed_text(&app, 140, 42);
    let offline_txt = render_buffer_text(&app, 140, 42);
    assert!(offline_sidebar.contains("Can't reach the daemon"));
    assert!(offline_txt.contains("SaintHarness server is not reachable"));

    app.open_model_popover();
    app.expanded_providers.clear();
    app.toggle_provider_expand("gemini");
    assert!(render_buffer_text(&app, 140, 42).contains("Can't load the Google catalog"));

    // Required UI states: sessionList empty & boardColumn empty
    app.daemon_state = DaemonConnectionState::Ok;
    app.model_popover_open = false;
    app.sessions.clear();
    assert!(render_buffer_text(&app, 140, 42).contains("No sessions yet"));
    app.board_open = true;
    let empty_board = render_buffer_text(&app, 140, 42);
    assert!(empty_board.contains("Nothing is waiting on you"));
    assert!(empty_board.contains("No session is running"));
    assert!(empty_board.contains("Finished sessions show up here"));

    // Reduced motion stops wave animation immediately
    let reduced = TuiApp::new(ThemeMode::SaintDark, 120, 38, true);
    assert!(!reduced.is_landing_wave_active());
}

#[test]
fn parity_live_sse_streaming_and_daemon_action_dispatch() {
    use saint_harness_protocol::SseWireEvent;
    use saint_harness_tui::DaemonAction;

    let mut app = TuiApp::new(ThemeMode::SaintDark, 140, 42, false);
    app.live_connected = true;

    app.apply_sse_event(&SseWireEvent {
        stream_id: 1,
        event_type: "session.created".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({
            "title": "live-sse-session",
            "model_profile": "claude-flagship"
        }),
    });
    app.select_session("live_1");

    app.set_composer_input("Fix lock bug".into());
    app.submit_composer();
    let actions = app.drain_actions();
    assert_eq!(actions.len(), 1);
    assert!(matches!(
        &actions[0],
        DaemonAction::SendPrompt { session_id, text, .. }
            if session_id == "live_1" && text == "Fix lock bug"
    ));

    app.apply_sse_event(&SseWireEvent {
        stream_id: 2,
        event_type: "tool.progress".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({ "tool_name": "read", "status": "running" }),
    });
    app.apply_sse_event(&SseWireEvent {
        stream_id: 3,
        event_type: "tool.progress".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({ "tool_name": "read", "status": "done ✓" }),
    });
    app.apply_sse_event(&SseWireEvent {
        stream_id: 4,
        event_type: "text.delta".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({ "delta": "Patched WAL lock cleanly." }),
    });
    app.apply_sse_event(&SseWireEvent {
        stream_id: 5,
        event_type: "turn.diff.updated".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({ "files_changed": 1, "lines_added": 7, "lines_removed": 2 }),
    });
    app.apply_sse_event(&SseWireEvent {
        stream_id: 6,
        event_type: "step.ended".into(),
        session_id: "live_1".into(),
        payload: serde_json::json!({
            "usage": { "input_tokens": 1000, "cache_read_input_tokens": 800 },
            "cost_usd": 0.003
        }),
    });

    let rendered = render_buffer_text(&app, 140, 42);
    assert!(rendered.contains("Patched WAL lock cleanly."));
    assert!(rendered.contains("+7-2"));
    assert!(rendered.contains("Warm · 80%"));
}

#[test]
fn interactive_keyboard_and_mouse_navigation_across_all_views() {
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use saint_harness_protocol::WireInteractionMode;
    use saint_harness_tui::{handle_key, handle_mouse, view::UiHitMap};

    let mut app = TuiApp::new(ThemeMode::SaintDark, 140, 42, false);

    // 1. Composer cursor left/right editing
    handle_key(&mut app, KeyCode::Char('h'), KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Char('i'), KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Left, KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Char('!'), KeyModifiers::NONE, 140);
    assert_eq!(app.composer_input, "h!i");
    handle_key(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL, 140);
    assert!(app.composer_input.is_empty());

    // 2. Model Picker keyboard navigation (Alt+] -> expand Claude -> pick Auto)
    handle_key(&mut app, KeyCode::Char(']'), KeyModifiers::ALT, 140);
    assert!(app.model_popover_open);
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, 140); // qwen4
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, 140); // Claude provider
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140); // expands & loads Claude
    assert!(app.expanded_providers.contains("claude"));
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, 140); // Claude Auto
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140); // selects Claude Flagship
    assert!(!app.model_popover_open);
    assert_eq!(
        app.current_session().unwrap().model_label,
        "Claude Flagship"
    );

    // 3. Access mode popover keyboard navigation (Alt+A -> Down -> Enter => Auto mode)
    handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::ALT, 140);
    assert!(app.access_popover_open);
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140);
    assert!(!app.access_popover_open);
    assert_eq!(
        app.current_session().unwrap().runtime_mode,
        WireRuntimeMode::Auto
    );

    // 4. /btw sidechat captures typing separately from composer
    handle_key(
        &mut app,
        KeyCode::Char('s'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
        140,
    );
    assert!(app.btw_open);
    handle_key(&mut app, KeyCode::Char('w'), KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Char('a'), KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Char('l'), KeyModifiers::NONE, 140);
    assert_eq!(app.btw_input, "wal");
    assert!(app.composer_input.is_empty());
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140);
    assert!(app.btw_output.contains("wal"));
    handle_key(&mut app, KeyCode::Esc, KeyModifiers::NONE, 140);
    assert!(!app.btw_open);

    // 5. Agent Board keyboard selection (Ctrl+G -> Down -> Enter)
    let before = app.active_session_id.clone();
    handle_key(&mut app, KeyCode::Char('g'), KeyModifiers::CONTROL, 140);
    assert!(app.board_open);
    handle_key(&mut app, KeyCode::Down, KeyModifiers::NONE, 140);
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140);
    assert!(!app.board_open);
    assert_ne!(app.active_session_id, before);

    // 6. Mouse hit-testing via render_with_hitmap
    let backend = TestBackend::new(140, 42);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut hitmap = UiHitMap::default();
    terminal
        .draw(|f| view::render_with_hitmap(f, &app, &mut hitmap))
        .unwrap();

    // Click Build/Plan toggle in composer footer
    let mode_rect = hitmap.composer_mode_btn.unwrap();
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: mode_rect.x + 1,
            row: mode_rect.y,
            modifiers: KeyModifiers::NONE,
        },
        &hitmap,
        140,
    );
    assert_eq!(
        app.current_session().unwrap().interaction_mode,
        WireInteractionMode::Plan
    );

    // Click another session row in sidebar
    let (sb_rect, sb_id) = hitmap
        .sidebar_sessions
        .iter()
        .find(|(_, id)| id != &app.active_session_id)
        .unwrap()
        .clone();
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: sb_rect.x + 1,
            row: sb_rect.y,
            modifiers: KeyModifiers::NONE,
        },
        &hitmap,
        140,
    );
    assert_eq!(app.active_session_id, sb_id);

    // /quit exits cleanly
    app.set_composer_input("/quit".into());
    handle_key(&mut app, KeyCode::Enter, KeyModifiers::NONE, 140);
    assert!(app.should_quit);
}
