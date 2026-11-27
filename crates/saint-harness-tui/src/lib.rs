pub mod app;
pub mod brand;
pub mod layout;
pub mod theme;
pub mod view;

pub use app::*;
pub use brand::*;
pub use layout::*;
pub use theme::*;

use crate::view::{OverlayHit, UiHitMap};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use saint_harness_protocol::{
    BtwRequest, BtwResponse, CreateSessionRequest, CreateSessionResponse, ForkRequest,
    PermissionReplyRequest, PromptRequest, SessionSummaryDto, SseWireEvent, WireInteractionMode,
    WireRuntimeMode,
};
use std::io;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct TuiLaunchOptions {
    pub attach_url: Option<String>,
    pub session_id: Option<String>,
    pub theme_mode: ThemeMode,
    pub reduced_motion: bool,
    pub default_profile: String,
    pub workspace_cwd: String,
}

impl Default for TuiLaunchOptions {
    fn default() -> Self {
        Self {
            attach_url: None,
            session_id: None,
            theme_mode: ThemeMode::SaintDark,
            reduced_motion: false,
            default_profile: "qwen-3.8-27b".to_string(),
            workspace_cwd: ".".to_string(),
        }
    }
}

enum DaemonUiUpdate {
    SessionCreated(Box<SessionSummaryDto>),
    BtwAnswer(String),
    Notice { session_id: String, message: String },
}

pub async fn run_tui(opts: TuiLaunchOptions) -> Result<(), String> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 38));
    let mut app = TuiApp::new(opts.theme_mode, cols, rows, opts.reduced_motion);
    app.default_model_profile = opts.default_profile.clone();
    if let Some(name) = Path::new(&opts.workspace_cwd)
        .file_name()
        .and_then(|s| s.to_str())
        && !name.is_empty()
    {
        app.workspace_label = name.to_string();
    }

    let target_url = opts
        .attach_url
        .clone()
        .unwrap_or_else(|| app.daemon_url.clone());
    let http = reqwest::Client::new();

    let (sse_tx, mut sse_rx) = mpsc::unbounded_channel::<SseWireEvent>();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<DaemonUiUpdate>();
    let mut live_url: Option<String> = None;

    if let Ok(resp) = http
        .get(format!("{}/sessions", target_url.trim_end_matches('/')))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        && resp.status().is_success()
    {
        app.daemon_url = target_url.clone();
        app.daemon_state = DaemonConnectionState::Ok;
        app.live_connected = true;
        live_url = Some(target_url.clone());

        let mut sessions = resp
            .json::<Vec<SessionSummaryDto>>()
            .await
            .unwrap_or_default();
        if sessions.is_empty()
            && let Ok(create_resp) = http
                .post(format!("{}/sessions", target_url.trim_end_matches('/')))
                .json(&CreateSessionRequest {
                    title: Some("new session".to_string()),
                    cwd: opts.workspace_cwd.clone(),
                    additional_directories: Vec::new(),
                    model_profile: opts.default_profile.clone(),
                    runtime_mode: WireRuntimeMode::ApprovalRequired,
                    interaction_mode: WireInteractionMode::Build,
                    sandbox_mode: None,
                    disable_internal_git_snapshot: false,
                    mcp_servers: Vec::new(),
                })
                .timeout(Duration::from_secs(3))
                .send()
                .await
            && create_resp.status().is_success()
            && let Ok(created) = create_resp.json::<CreateSessionResponse>().await
        {
            sessions.push(created.session);
        }

        if !sessions.is_empty() {
            app.active_session_id = opts
                .session_id
                .clone()
                .filter(|sid| sessions.iter().any(|s| &s.id == sid))
                .unwrap_or_else(|| sessions[0].id.clone());
            app.sessions = sessions;
        }
    } else if opts.attach_url.is_some() {
        app.daemon_url = target_url;
        app.daemon_state = DaemonConnectionState::Error;
    } else if let Some(sid) = opts.session_id {
        app.select_session(&sid);
    }

    let sse_task = live_url.as_ref().map(|base| {
        let events_url = format!("{}/events", base.trim_end_matches('/'));
        let client = http.clone();
        let tx = sse_tx.clone();
        tokio::spawn(async move {
            if let Ok(resp) = client.get(&events_url).send().await
                && resp.status().is_success()
            {
                let mut stream = resp.bytes_stream().eventsource();
                while let Some(Ok(ev)) = stream.next().await {
                    if let Ok(wire) = serde_json::from_str::<SseWireEvent>(&ev.data) {
                        let _ = tx.send(wire);
                    }
                }
            }
        })
    });

    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let loop_res = run_event_loop(
        &mut terminal,
        &mut app,
        &mut sse_rx,
        &mut ui_rx,
        &ui_tx,
        live_url.as_deref(),
        &opts.workspace_cwd,
        &http,
    );

    if let Some(handle) = sse_task {
        handle.abort();
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    loop_res
}

fn dispatch_daemon_action(
    base_url: &str,
    workspace_cwd: &str,
    client: &reqwest::Client,
    ui_tx: &mpsc::UnboundedSender<DaemonUiUpdate>,
    action: DaemonAction,
) {
    let base = base_url.trim_end_matches('/').to_string();
    let cwd = workspace_cwd.to_string();
    let http = client.clone();
    let tx = ui_tx.clone();
    tokio::spawn(async move {
        match action {
            DaemonAction::SendPrompt {
                session_id,
                text,
                delivery,
                profile_override,
                model_override,
                interaction_mode,
                runtime_mode,
            } => {
                match http
                    .post(format!("{base}/sessions/{session_id}/prompt"))
                    .json(&PromptRequest {
                        text,
                        delivery,
                        client_msg_id: None,
                        profile_override,
                        model_override,
                        interaction_mode,
                        runtime_mode,
                    })
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {}
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        let msg = if body.trim().is_empty() {
                            format!("prompt request failed ({status})")
                        } else {
                            format!("prompt request failed ({status}): {}", body.trim())
                        };
                        let _ = tx.send(DaemonUiUpdate::Notice {
                            session_id,
                            message: msg,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(DaemonUiUpdate::Notice {
                            session_id,
                            message: format!("daemon request failed: {e}"),
                        });
                    }
                }
            }
            DaemonAction::ReplyPermission { request_id, choice } => {
                let decision = match choice {
                    1 => "once",
                    2 => "always",
                    3 => "reject_with_feedback",
                    _ => "reject",
                };
                let _ = http
                    .post(format!("{base}/permissions/{request_id}/reply"))
                    .json(&PermissionReplyRequest {
                        decision: decision.to_string(),
                        pattern: None,
                        feedback: None,
                    })
                    .send()
                    .await;
            }
            DaemonAction::Interrupt { session_id } => {
                let _ = http
                    .post(format!("{base}/sessions/{session_id}/interrupt"))
                    .send()
                    .await;
            }
            DaemonAction::CreateSession {
                title,
                model_profile,
            } => {
                if let Ok(resp) = http
                    .post(format!("{base}/sessions"))
                    .json(&CreateSessionRequest {
                        title: Some(title),
                        cwd,
                        additional_directories: Vec::new(),
                        model_profile,
                        runtime_mode: WireRuntimeMode::ApprovalRequired,
                        interaction_mode: WireInteractionMode::Build,
                        sandbox_mode: None,
                        disable_internal_git_snapshot: false,
                        mcp_servers: Vec::new(),
                    })
                    .send()
                    .await
                    && resp.status().is_success()
                    && let Ok(created) = resp.json::<CreateSessionResponse>().await
                {
                    let _ = tx.send(DaemonUiUpdate::SessionCreated(Box::new(created.session)));
                }
            }
            DaemonAction::SendBtw {
                session_id,
                question,
            } => {
                match http
                    .post(format!("{base}/sessions/{session_id}/btw"))
                    .json(&BtwRequest { question })
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        if let Ok(ans) = resp.json::<BtwResponse>().await {
                            let _ = tx.send(DaemonUiUpdate::BtwAnswer(ans.answer));
                        }
                    }
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        let _ = tx.send(DaemonUiUpdate::BtwAnswer(format!(
                            "btw request failed ({status}): {}",
                            body.trim()
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(DaemonUiUpdate::BtwAnswer(format!(
                            "btw request failed: {e}"
                        )));
                    }
                }
            }
            DaemonAction::ForkSession {
                session_id,
                through_msg_id,
            } => {
                if let Ok(resp) = http
                    .post(format!("{base}/sessions/{session_id}/fork"))
                    .json(&ForkRequest { through_msg_id })
                    .send()
                    .await
                    && resp.status().is_success()
                    && let Ok(body) = resp.json::<serde_json::Value>().await
                    && let Some(new_id) = body["session_id"].as_str()
                    && let Ok(get_resp) = http.get(format!("{base}/sessions/{new_id}")).send().await
                    && get_resp.status().is_success()
                    && let Ok(sess) = get_resp.json::<SessionSummaryDto>().await
                {
                    let _ = tx.send(DaemonUiUpdate::SessionCreated(Box::new(sess)));
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
    sse_rx: &mut mpsc::UnboundedReceiver<SseWireEvent>,
    ui_rx: &mut mpsc::UnboundedReceiver<DaemonUiUpdate>,
    ui_tx: &mpsc::UnboundedSender<DaemonUiUpdate>,
    live_url: Option<&str>,
    workspace_cwd: &str,
    http: &reqwest::Client,
) -> Result<(), String> {
    let tick_rate = Duration::from_millis(SPINNER_INTERVAL_MS);
    let mut hitmap = UiHitMap::default();
    loop {
        while let Ok(wire) = sse_rx.try_recv() {
            app.apply_sse_event(&wire);
        }
        while let Ok(upd) = ui_rx.try_recv() {
            match upd {
                DaemonUiUpdate::SessionCreated(sess) => {
                    let sess = *sess;
                    let id = sess.id.clone();
                    if let Some(existing) = app.sessions.iter_mut().find(|s| s.id == id) {
                        *existing = sess;
                    } else {
                        app.sessions.push(sess);
                    }
                    app.select_session(&id);
                }
                DaemonUiUpdate::BtwAnswer(ans) => {
                    app.btw_output = ans;
                }
                DaemonUiUpdate::Notice {
                    session_id,
                    message,
                } => {
                    if let Some(sess) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                        sess.status = saint_harness_protocol::SessionUiStatus::Done;
                        sess.subtitle = "error".to_string();
                        sess.messages
                            .push(saint_harness_protocol::TranscriptItemDto::Handoff {
                                text: message,
                            });
                    }
                }
            }
        }
        if let Some(base) = live_url {
            for action in app.drain_actions() {
                dispatch_daemon_action(base, workspace_cwd, http, ui_tx, action);
            }
        }
        terminal
            .draw(|f| view::render_with_hitmap(f, app, &mut hitmap))
            .map_err(|e| e.to_string())?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(tick_rate).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Resize(cols, rows) => app.on_resize(cols, rows),
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let cols = terminal.size().map(|s| s.width).unwrap_or(120);
                    handle_key(app, key.code, key.modifiers, cols);
                }
                Event::Mouse(mouse) => {
                    let cols = terminal.size().map(|s| s.width).unwrap_or(120);
                    handle_mouse(app, mouse, &hitmap, cols);
                }
                _ => {}
            }
        } else {
            app.tick(SPINNER_INTERVAL_MS);
        }
    }
}

pub fn handle_key(app: &mut TuiApp, code: KeyCode, mods: KeyModifiers, cols: u16) {
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let alt = mods.contains(KeyModifiers::ALT);
    let shift = mods.contains(KeyModifiers::SHIFT);

    if ctrl && matches!(code, KeyCode::Char('c') | KeyCode::Char('q')) {
        app.should_quit = true;
        return;
    }
    if ctrl && alt && code == KeyCode::Char('s') {
        app.open_btw();
        return;
    }
    if ctrl && shift && matches!(code, KeyCode::Char('M') | KeyCode::Char('m')) {
        app.open_model_popover();
        return;
    }
    if ctrl {
        match code {
            KeyCode::Char('b') => {
                app.toggle_sidebar();
                return;
            }
            KeyCode::Char('d') => {
                app.toggle_diff_drawer();
                return;
            }
            KeyCode::Char('g') => {
                app.toggle_board();
                return;
            }
            KeyCode::Char('j') => {
                app.toggle_cli_drawer();
                return;
            }
            KeyCode::Char('t') => {
                app.open_tree_modal();
                return;
            }
            KeyCode::Char('o') => {
                app.toggle_latest_worked_for();
                return;
            }
            KeyCode::Char('n') => {
                app.request_new_session();
                return;
            }
            KeyCode::Char('u') => {
                app.clear_composer();
                return;
            }
            KeyCode::Up => {
                app.select_adjacent_session(-1);
                return;
            }
            KeyCode::Down => {
                app.select_adjacent_session(1);
                return;
            }
            _ => {}
        }
    }
    if alt {
        match code {
            KeyCode::Char('m') | KeyCode::Char('M') | KeyCode::Char(']') => {
                app.open_model_popover();
                return;
            }
            KeyCode::Char('a') => {
                app.open_access_popover();
                return;
            }
            KeyCode::Char('t') => {
                app.toggle_theme();
                return;
            }
            KeyCode::Up => {
                app.select_adjacent_session(-1);
                return;
            }
            KeyCode::Down => {
                app.select_adjacent_session(1);
                return;
            }
            _ => {}
        }
    }

    if code == KeyCode::Esc {
        app.handle_escape(cols);
        return;
    }

    // Context-aware modal/popover keyboard routing (top-most layer wins).
    if app.tree_modal_open {
        match code {
            KeyCode::Up => app.tree_move(-1),
            KeyCode::Down => app.tree_move(1),
            KeyCode::Enter => app.tree_activate(),
            KeyCode::Char('f') | KeyCode::Char('F') => app.tree_fork_head(),
            _ => {}
        }
        return;
    }

    if app.btw_open {
        match code {
            KeyCode::Enter => app.btw_submit(),
            KeyCode::Backspace => app.btw_backspace(),
            KeyCode::Char(c) => app.btw_push_char(c),
            _ => {}
        }
        return;
    }

    if app.access_popover_open {
        match code {
            KeyCode::Up => app.access_popover_move(-1),
            KeyCode::Down => app.access_popover_move(1),
            KeyCode::Enter | KeyCode::Char(' ') => app.access_popover_activate(),
            _ => {}
        }
        return;
    }

    if app.model_popover_open {
        match code {
            KeyCode::Up => app.model_picker_move(-1),
            KeyCode::Down => app.model_picker_move(1),
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => app.model_picker_activate(),
            KeyCode::Left => app.model_picker_collapse_current(),
            _ => {}
        }
        return;
    }

    if app.cache_popover_open {
        if matches!(code, KeyCode::Enter | KeyCode::Char(' ')) {
            app.handle_escape(cols);
        }
        return;
    }

    if app.board_open {
        match code {
            KeyCode::Up | KeyCode::Left => app.board_move(-1),
            KeyCode::Down | KeyCode::Right | KeyCode::Tab => app.board_move(1),
            KeyCode::BackTab => app.board_move(-1),
            KeyCode::Enter => app.board_activate(),
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::BackTab => {
            app.toggle_interaction_mode();
        }
        KeyCode::Tab => {
            if app.inline_palette_open {
                app.accept_inline_palette_selection();
            } else if app.cli_drawer_open && app.composer_input.is_empty() {
                app.cycle_cli_tab();
            }
        }
        KeyCode::Up if app.inline_palette_open => {
            app.palette_move(-1);
        }
        KeyCode::Down if app.inline_palette_open => {
            app.palette_move(1);
        }
        KeyCode::PageUp => {
            app.scroll_transcript_up(6);
        }
        KeyCode::PageDown => {
            app.scroll_transcript_down(6);
        }
        KeyCode::Left => {
            app.move_composer_left();
        }
        KeyCode::Right => {
            app.move_composer_right();
        }
        KeyCode::Home => {
            app.move_composer_home();
        }
        KeyCode::End => {
            app.move_composer_end();
        }
        KeyCode::Enter => {
            app.submit_composer();
        }
        KeyCode::Backspace => {
            app.backspace_composer();
        }
        KeyCode::Delete => {
            app.delete_composer();
        }
        KeyCode::Char(c @ '1'..='4') if app.composer_input.is_empty() => {
            let choice = (c as u8) - b'0';
            if !app.resolve_approval_choice(choice) {
                app.insert_composer_char(c);
            }
        }
        KeyCode::Char(c) => {
            app.insert_composer_char(c);
        }
        _ => {}
    }
}

pub fn handle_mouse(app: &mut TuiApp, mouse: MouseEvent, hitmap: &UiHitMap, cols: u16) {
    let col = mouse.column;
    let row = mouse.row;
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            app.scroll_transcript_up(3);
            return;
        }
        MouseEventKind::ScrollDown => {
            app.scroll_transcript_down(3);
            return;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return,
    }

    // 1. Check active overlay/board hit items first.
    for (rect, hit) in &hitmap.overlay_hits {
        if UiHitMap::contains(*rect, col, row) {
            match hit {
                OverlayHit::ModelRow(idx) => {
                    app.model_picker_idx = *idx;
                    app.model_picker_activate();
                }
                OverlayHit::AccessMode(mode) => {
                    app.set_runtime_mode(*mode);
                }
                OverlayHit::PaletteRow(idx) => {
                    app.inline_palette_idx = *idx;
                    app.accept_inline_palette_selection();
                }
                OverlayHit::TreeNode(idx) => {
                    app.tree_selected_idx = *idx;
                    app.tree_activate();
                }
                OverlayHit::TreeForkHead => {
                    app.tree_fork_head();
                }
                OverlayHit::BoardSession(sid) => {
                    app.select_session(sid);
                }
                OverlayHit::BoardBack => {
                    app.board_open = false;
                }
                OverlayHit::CliTab(tab) => {
                    app.cli_tab = *tab;
                }
            }
            return;
        }
    }

    // Click outside an open floating popover dismisses it.
    if let Some(ov_rect) = hitmap.overlay_rect
        && !UiHitMap::contains(ov_rect, col, row)
        && (app.model_popover_open
            || app.access_popover_open
            || app.cache_popover_open
            || app.tree_modal_open
            || app.btw_open
            || app.inline_palette_open)
    {
        app.handle_escape(cols);
        return;
    }

    // 2. Header controls
    if hitmap
        .header_sidebar_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_sidebar();
        return;
    }
    if hitmap
        .header_model_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.open_model_popover();
        return;
    }
    if hitmap
        .header_diff_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_diff_drawer();
        return;
    }
    if hitmap
        .header_cache_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.open_cache_popover();
        return;
    }

    // 3. Sidebar controls & sessions
    if hitmap
        .sidebar_new_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.request_new_session();
        return;
    }
    if hitmap
        .sidebar_board_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_board();
        return;
    }
    for (rect, sid) in &hitmap.sidebar_sessions {
        if UiHitMap::contains(*rect, col, row) {
            app.select_session(sid);
            return;
        }
    }

    // 4. Diff drawer header toggles #hash / line numbers
    if hitmap
        .diff_hashline_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_diff_hashline();
        return;
    }

    // 5. Approval card choices & sandbox details
    if hitmap
        .approval_sandbox_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_sandbox_details();
        return;
    }
    for (rect, choice) in &hitmap.approval_choices {
        if UiHitMap::contains(*rect, col, row) {
            app.resolve_approval_choice(*choice);
            return;
        }
    }

    // 6. Composer footer buttons & focus
    if hitmap
        .composer_mode_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_interaction_mode();
        return;
    }
    if hitmap
        .composer_access_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.open_access_popover();
        return;
    }
    if hitmap
        .composer_send_btn
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.submit_composer();
        return;
    }
    if hitmap
        .composer_area
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.focus = FocusTarget::Composer;
        return;
    }

    // 7. Landing starter prompt chips
    for (rect, prompt) in &hitmap.landing_prompts {
        if UiHitMap::contains(*rect, col, row) {
            app.set_composer_input(prompt.clone());
            app.submit_composer();
            return;
        }
    }

    // 8. Click inside transcript toggles latest `Worked for...` if present
    if hitmap
        .transcript_area
        .is_some_and(|r| UiHitMap::contains(r, col, row))
    {
        app.toggle_latest_worked_for();
    }
}
