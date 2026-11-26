pub mod app;
pub mod brand;
pub mod layout;
pub mod theme;
pub mod view;

pub use app::*;
pub use brand::*;
pub use layout::*;
pub use theme::*;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use saint_harness_protocol::{PermissionReplyRequest, PromptRequest, SseWireEvent};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct TuiLaunchOptions {
    pub attach_url: Option<String>,
    pub session_id: Option<String>,
    pub theme_mode: ThemeMode,
    pub reduced_motion: bool,
}

impl Default for TuiLaunchOptions {
    fn default() -> Self {
        Self {
            attach_url: None,
            session_id: None,
            theme_mode: ThemeMode::SaintDark,
            reduced_motion: false,
        }
    }
}

pub async fn run_tui(opts: TuiLaunchOptions) -> Result<(), String> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((120, 38));
    let mut app = TuiApp::new(opts.theme_mode, cols, rows, opts.reduced_motion);
    let target_url = opts
        .attach_url
        .clone()
        .unwrap_or_else(|| app.daemon_url.clone());
    let http = reqwest::Client::new();

    let (sse_tx, mut sse_rx) = mpsc::unbounded_channel::<SseWireEvent>();
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
        if let Ok(sessions) = resp
            .json::<Vec<saint_harness_protocol::SessionSummaryDto>>()
            .await
            && !sessions.is_empty()
        {
            app.active_session_id = opts
                .session_id
                .clone()
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
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| e.to_string())?;

    let loop_res = run_event_loop(
        &mut terminal,
        &mut app,
        &mut sse_rx,
        live_url.as_deref(),
        &http,
    );

    if let Some(handle) = sse_task {
        handle.abort();
    }
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    loop_res
}

fn dispatch_daemon_action(base_url: &str, client: &reqwest::Client, action: DaemonAction) {
    let base = base_url.trim_end_matches('/').to_string();
    let http = client.clone();
    tokio::spawn(async move {
        match action {
            DaemonAction::SendPrompt {
                session_id,
                text,
                delivery,
            } => {
                let _ = http
                    .post(format!("{base}/sessions/{session_id}/prompt"))
                    .json(&PromptRequest {
                        text,
                        delivery,
                        client_msg_id: None,
                        model_override: None,
                        interaction_mode: None,
                    })
                    .send()
                    .await;
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
        }
    });
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
    sse_rx: &mut mpsc::UnboundedReceiver<SseWireEvent>,
    live_url: Option<&str>,
    http: &reqwest::Client,
) -> Result<(), String> {
    let tick_rate = Duration::from_millis(SPINNER_INTERVAL_MS);
    loop {
        while let Ok(wire) = sse_rx.try_recv() {
            app.apply_sse_event(&wire);
        }
        if let Some(base) = live_url {
            for action in app.drain_actions() {
                dispatch_daemon_action(base, http, action);
            }
        }
        terminal
            .draw(|f| view::render(f, app))
            .map_err(|e| e.to_string())?;
        if app.should_quit {
            return Ok(());
        }
        if event::poll(tick_rate).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Resize(cols, rows) => app.on_resize(cols, rows),
                Event::Key(key) => {
                    let cols = terminal.size().map(|s| s.width).unwrap_or(120);
                    handle_key(app, key.code, key.modifiers, cols);
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

    if ctrl && code == KeyCode::Char('c') {
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
                app.select_session("idle-new");
                return;
            }
            _ => {}
        }
    }
    if alt && code == KeyCode::Char(']') {
        app.open_model_popover();
        return;
    }

    match code {
        KeyCode::Esc => {
            app.handle_escape(cols);
        }
        KeyCode::BackTab => {
            app.toggle_interaction_mode();
        }
        KeyCode::Up if app.inline_palette_open => {
            app.palette_move(-1);
        }
        KeyCode::Down if app.inline_palette_open => {
            app.palette_move(1);
        }
        KeyCode::Enter => {
            app.submit_composer();
        }
        KeyCode::Backspace => {
            let mut next = app.composer_input.clone();
            next.pop();
            app.set_composer_input(next);
        }
        KeyCode::Char(c @ '1'..='4') if app.composer_input.is_empty() => {
            let choice = (c as u8) - b'0';
            if !app.resolve_approval_choice(choice) {
                let mut next = app.composer_input.clone();
                next.push(c);
                app.set_composer_input(next);
            }
        }
        KeyCode::Char(c) => {
            let mut next = app.composer_input.clone();
            next.push(c);
            app.set_composer_input(next);
        }
        _ => {}
    }
}
