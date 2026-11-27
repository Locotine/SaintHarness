use crate::app::{CatalogLoadState, CliTab, DaemonConnectionState, ModelPickerRow, TuiApp};
use crate::layout::{DiffDrawerMode, ResponsiveLayoutSpec, resolve_layout};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use saint_harness_protocol::{
    DecisionCardDto, SessionSummaryDto, SessionUiStatus, TranscriptItemDto, WireInteractionMode,
    WireRuntimeMode,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayHit {
    ModelRow(usize),
    AccessMode(WireRuntimeMode),
    PaletteRow(usize),
    TreeNode(usize),
    TreeForkHead,
    BoardSession(String),
    BoardBack,
    CliTab(CliTab),
}

#[derive(Debug, Default, Clone)]
pub struct UiHitMap {
    pub header_sidebar_btn: Option<Rect>,
    pub header_cache_btn: Option<Rect>,
    pub header_diff_btn: Option<Rect>,
    pub header_model_btn: Option<Rect>,
    pub sidebar_new_btn: Option<Rect>,
    pub sidebar_board_btn: Option<Rect>,
    pub sidebar_sessions: Vec<(Rect, String)>,
    pub landing_prompts: Vec<(Rect, String)>,
    pub transcript_area: Option<Rect>,
    pub approval_sandbox_btn: Option<Rect>,
    pub approval_choices: Vec<(Rect, u8)>,
    pub composer_area: Option<Rect>,
    pub composer_mode_btn: Option<Rect>,
    pub composer_access_btn: Option<Rect>,
    pub composer_send_btn: Option<Rect>,
    pub diff_hashline_btn: Option<Rect>,
    pub overlay_rect: Option<Rect>,
    pub overlay_hits: Vec<(Rect, OverlayHit)>,
}

impl UiHitMap {
    pub fn contains(rect: Rect, col: u16, row: u16) -> bool {
        col >= rect.x
            && col < rect.x.saturating_add(rect.width)
            && row >= rect.y
            && row < rect.y.saturating_add(rect.height)
    }
}

pub fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let mut hitmap = UiHitMap::default();
    render_with_hitmap(frame, app, &mut hitmap);
}

pub fn render_with_hitmap(frame: &mut Frame<'_>, app: &TuiApp, hitmap: &mut UiHitMap) {
    *hitmap = UiHitMap::default();
    let area = frame.area();
    let spec = resolve_layout(area.width, area.height);
    let t = &app.theme;

    // Paint root surface background
    frame.render_widget(
        Block::default().style(Style::default().bg(t.surface).fg(t.ink_primary)),
        area,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(6)])
        .split(area);

    render_header(frame, rows[0], app, &spec, hitmap);

    let body = rows[1];
    let split_sidebar = app.sidebar_open && spec.diff_drawer_mode == DiffDrawerMode::Split;
    let split_diff = app.diff_drawer_open && spec.diff_drawer_mode == DiffDrawerMode::Split;

    let mut constraints = Vec::new();
    if split_sidebar {
        constraints.push(Constraint::Length(spec.sidebar_cols.min(body.width / 3)));
    }
    constraints.push(Constraint::Min(28));
    if split_diff {
        constraints.push(Constraint::Length(
            spec.diff_drawer_cols.min(body.width / 2),
        ));
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(body);

    let mut col_idx = 0usize;
    if split_sidebar {
        render_sidebar(frame, cols[col_idx], app, hitmap);
        col_idx += 1;
    }
    let main_area = cols[col_idx];
    col_idx += 1;

    render_main_column(frame, main_area, app, &spec, hitmap);

    if split_diff {
        render_diff_drawer(frame, cols[col_idx], app, hitmap);
    }

    // Narrow/ultra_compact overlay rails
    if app.sidebar_open && !split_sidebar {
        let overlay = Rect {
            x: body.x,
            y: body.y,
            width: spec.sidebar_cols.min(body.width.saturating_sub(2)),
            height: body.height,
        };
        frame.render_widget(Clear, overlay);
        render_sidebar(frame, overlay, app, hitmap);
    }
    if app.diff_drawer_open && !split_diff {
        frame.render_widget(Clear, body);
        render_diff_drawer(frame, body, app, hitmap);
    }

    // Floating popovers & modals (top-most layers closed one-by-one by Escape)
    if app.cache_popover_open {
        render_cache_popover(frame, body, app, hitmap);
    }
    if app.model_popover_open {
        render_model_popover(frame, body, app, hitmap);
    }
    if app.access_popover_open {
        render_access_popover(frame, main_area, app, hitmap);
    }
    if app.inline_palette_open {
        render_inline_palette(frame, main_area, app, hitmap);
    }
    if app.btw_open {
        render_btw_sheet(frame, body, app, hitmap);
    }
    if app.tree_modal_open {
        render_tree_modal(frame, body, app, hitmap);
    }
}

fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    spec: &ResponsiveLayoutSpec,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let sess = app.current_session();
    let sess_name = sess.map(|s| s.name.as_str()).unwrap_or("SaintHarness");

    let mut left_spans = vec![
        Span::styled(" ◧ ", Style::default().fg(t.ink_secondary)),
        Span::styled(
            "SaintHarness",
            Style::default()
                .fg(t.ink_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::default().fg(t.ink_tertiary)),
        Span::styled(sess_name, Style::default().fg(t.ink_secondary)),
    ];

    // Right glance items: strictly capped by `spec.max_header_glance_items` (2 on ultra_compact, 3 on >=100 cols).
    let mut right_spans: Vec<Span<'_>> = Vec::new();
    let mut cache_w = 0u16;
    let mut diff_w = 0u16;
    let mut model_w = 0u16;
    if let Some(s) = sess {
        if spec.max_header_glance_items >= 3 {
            let dot_color = if s.cache_warm {
                t.status_done
            } else {
                t.ink_tertiary
            };
            let cache_txt = format!("{}  ", s.cache_label);
            cache_w = (2 + cache_txt.chars().count()) as u16;
            right_spans.push(Span::styled("● ", Style::default().fg(dot_color)));
            right_spans.push(Span::styled(
                cache_txt,
                Style::default().fg(t.ink_secondary),
            ));
        }
        if let Some(ref d) = s.diff {
            let add_txt = format!("+{}", d.added);
            let rem_txt = format!("-{}  ", d.removed);
            diff_w = (add_txt.chars().count() + rem_txt.chars().count()) as u16;
            right_spans.push(Span::styled(
                add_txt,
                Style::default().fg(t.status_done_ink),
            ));
            right_spans.push(Span::styled(
                rem_txt,
                Style::default().fg(t.status_error_ink),
            ));
        }
        let model_txt = format!("{} ▾ ", s.model_label);
        model_w = model_txt.chars().count() as u16;
        right_spans.push(Span::styled(model_txt, Style::default().fg(t.ink_primary)));
    }

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(18), Constraint::Min(18)])
        .split(area);

    hitmap.header_sidebar_btn = Some(Rect {
        x: chunks[0].x,
        y: chunks[0].y,
        width: 4.min(chunks[0].width),
        height: 1,
    });
    let right_end = chunks[1].x.saturating_add(chunks[1].width);
    if model_w > 0 {
        let mx = right_end.saturating_sub(model_w);
        hitmap.header_model_btn = Some(Rect {
            x: mx,
            y: chunks[1].y,
            width: model_w.min(chunks[1].width),
            height: 1,
        });
        if diff_w > 0 {
            let dx = mx.saturating_sub(diff_w);
            hitmap.header_diff_btn = Some(Rect {
                x: dx,
                y: chunks[1].y,
                width: diff_w,
                height: 1,
            });
            if cache_w > 0 {
                let cx = dx.saturating_sub(cache_w);
                hitmap.header_cache_btn = Some(Rect {
                    x: cx,
                    y: chunks[1].y,
                    width: cache_w,
                    height: 1,
                });
            }
        } else if cache_w > 0 {
            let cx = mx.saturating_sub(cache_w);
            hitmap.header_cache_btn = Some(Rect {
                x: cx,
                y: chunks[1].y,
                width: cache_w,
                height: 1,
            });
        }
    }

    left_spans.push(Span::raw(" "));
    frame.render_widget(
        Paragraph::new(Line::from(left_spans)).style(Style::default().bg(t.sidebar_bg)),
        chunks[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(right_spans))
            .alignment(Alignment::Right)
            .style(Style::default().bg(t.sidebar_bg)),
        chunks[1],
    );
}

fn render_sidebar(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.sidebar_bg).fg(t.ink_primary));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " SaintHarness",
                Style::default()
                    .fg(t.ink_secondary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  [+]", Style::default().fg(t.ink_tertiary)),
        ])),
        sections[0],
    );
    hitmap.sidebar_new_btn = Some(Rect {
        x: sections[0].x.saturating_add(13),
        y: sections[0].y,
        width: sections[0].width.saturating_sub(13),
        height: 1,
    });

    let mut lines: Vec<Line<'_>> = Vec::new();
    match app.daemon_state {
        DaemonConnectionState::Loading => {
            lines.push(Line::from(Span::styled(
                " Loading sessions from the daemon…",
                Style::default().fg(t.ink_secondary),
            )));
            lines.push(Line::from(Span::styled(
                " ░░░░░░░░░░░░░░░░░░",
                Style::default().fg(t.border_heavy),
            )));
            lines.push(Line::from(Span::styled(
                " ░░░░░░░░░░░░░░░░░░",
                Style::default().fg(t.border_heavy),
            )));
        }
        DaemonConnectionState::Error => {
            lines.push(Line::from(Span::styled(
                " Can't reach the daemon",
                Style::default()
                    .fg(t.ink_primary)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                " Start with `saint-harness server`",
                Style::default().fg(t.ink_secondary),
            )));
            lines.push(Line::from(Span::styled(
                " [ Retry connection ]",
                Style::default().bg(t.user_bubble).fg(t.ink_primary),
            )));
        }
        DaemonConnectionState::Ok => {
            let grouped = app.grouped_sessions();
            if grouped.is_empty() {
                lines.push(Line::from(Span::styled(
                    " No sessions yet",
                    Style::default()
                        .fg(t.ink_primary)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    " Press Ctrl+N or type below.",
                    Style::default().fg(t.ink_secondary),
                )));
            } else {
                let mut row_cursor: u16 = 0;
                for (status, items) in grouped {
                    lines.push(Line::from(Span::styled(
                        format!(
                            " {}  ({})",
                            status.group_label().to_ascii_uppercase(),
                            items.len()
                        ),
                        Style::default().fg(t.ink_tertiary),
                    )));
                    row_cursor = row_cursor.saturating_add(1);
                    for s in items {
                        let is_current = s.id == app.active_session_id && !app.board_open;
                        let row_bg = if is_current {
                            t.user_bubble
                        } else {
                            t.sidebar_bg
                        };
                        let (glyph, glyph_fg) = match s.status {
                            SessionUiStatus::Blocked => ("●", t.status_blocked),
                            SessionUiStatus::Working => (app.spinner_glyph(), t.status_working),
                            SessionUiStatus::Done => ("●", t.status_done),
                            SessionUiStatus::Idle => ("○", t.ink_tertiary),
                        };
                        let check = if is_current { " ✓" } else { "" };
                        let title_y = sections[1].y.saturating_add(row_cursor);
                        if row_cursor < sections[1].height {
                            hitmap.sidebar_sessions.push((
                                Rect {
                                    x: sections[1].x,
                                    y: title_y,
                                    width: sections[1].width,
                                    height: 2.min(sections[1].height.saturating_sub(row_cursor)),
                                },
                                s.id.clone(),
                            ));
                        }
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!(" {glyph} "),
                                Style::default().fg(glyph_fg).bg(row_bg),
                            ),
                            Span::styled(
                                format!("{}{check}", s.name),
                                Style::default().fg(t.ink_primary).bg(row_bg),
                            ),
                        ]));
                        let right_meta = if s.status == SessionUiStatus::Blocked {
                            Span::styled(
                                format!("   {} · {}", s.model_label, s.subtitle),
                                Style::default().fg(t.status_blocked_ink).bg(row_bg),
                            )
                        } else {
                            Span::styled(
                                format!("   {} · {}", s.model_label, s.subtitle),
                                Style::default().fg(t.ink_tertiary).bg(row_bg),
                            )
                        };
                        lines.push(Line::from(right_meta));
                        row_cursor = row_cursor.saturating_add(2);
                    }
                }
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        sections[1],
    );

    let footer = match app.daemon_state {
        DaemonConnectionState::Error => Line::from(vec![
            Span::styled(" ⊞ Board  ", Style::default().fg(t.ink_secondary)),
            Span::styled("● ", Style::default().fg(t.status_error)),
            Span::styled("daemon offline", Style::default().fg(t.status_error_ink)),
        ]),
        DaemonConnectionState::Loading => Line::from(vec![
            Span::styled(" ⊞ Board  ", Style::default().fg(t.ink_secondary)),
            Span::styled("○ connecting…", Style::default().fg(t.ink_tertiary)),
        ]),
        DaemonConnectionState::Ok => Line::from(vec![
            Span::styled(" ⊞ Board  ", Style::default().fg(t.ink_secondary)),
            Span::styled("● ", Style::default().fg(t.status_done)),
            Span::styled(
                if app.live_connected {
                    "wal · live"
                } else {
                    "wal · 11ms"
                },
                Style::default().fg(t.ink_tertiary),
            ),
        ]),
    };
    hitmap.sidebar_board_btn = Some(Rect {
        x: sections[2].x,
        y: sections[2].y,
        width: 10.min(sections[2].width),
        height: 1,
    });
    frame.render_widget(Paragraph::new(footer), sections[2]);
}

fn render_main_column(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    spec: &ResponsiveLayoutSpec,
    hitmap: &mut UiHitMap,
) {
    let split = if app.cli_drawer_open {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(10), Constraint::Length(8)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6)])
            .split(area)
    };

    if app.board_open {
        render_board(frame, split[0], app, spec, hitmap);
    } else {
        render_focus_session(frame, split[0], app, spec, hitmap);
    }

    if app.cli_drawer_open && split.len() > 1 {
        render_cli_drawer(frame, split[1], app, hitmap);
    }
}

fn render_focus_session(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    spec: &ResponsiveLayoutSpec,
    hitmap: &mut UiHitMap,
) {
    let sess = app.current_session();
    let has_decision =
        sess.is_some_and(|s| s.status == SessionUiStatus::Blocked && s.decision.is_some());
    let decision_height = if has_decision {
        if app.sandbox_details_open { 10 } else { 9 }
    } else {
        0
    };
    let steer_height = u16::from(app.steer_banner.is_some());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(decision_height),
            Constraint::Length(steer_height),
            Constraint::Length(4),
        ])
        .split(area);

    hitmap.transcript_area = Some(chunks[0]);
    if sess.is_none_or(|s| s.messages.is_empty()) {
        render_landing(frame, chunks[0], app, spec, hitmap);
    } else if let Some(s) = sess {
        render_transcript(frame, chunks[0], app, s);
    }

    if has_decision
        && let Some(s) = sess
        && let Some(ref dec) = s.decision
    {
        render_decision_card(frame, chunks[1], app, dec, hitmap);
    }

    if let Some(ref steer) = app.steer_banner {
        let t = &app.theme;
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {steer}"),
                Style::default().bg(t.accent_bg).fg(t.ink_primary),
            )))
            .style(Style::default().bg(t.accent_bg)),
            chunks[2],
        );
    }

    render_composer(frame, chunks[3], app, sess, hitmap);
}

fn render_landing(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    spec: &ResponsiveLayoutSpec,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let logo = spec.logo_variant;
    let mut lines: Vec<Line<'_>> = Vec::new();
    for (idx, line) in logo.lines().iter().enumerate() {
        let color = if logo.is_crown_row(idx) {
            t.accent
        } else {
            t.ink_primary
        };
        lines.push(Line::from(Span::styled(*line, Style::default().fg(color))));
    }
    lines.push(Line::from(Span::styled(
        "Let's build",
        Style::default()
            .fg(t.ink_primary)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("{} · main", app.workspace_label),
        Style::default().fg(t.ink_tertiary),
    )));
    if area.height >= 16 {
        let chip_y = area.y.saturating_add(lines.len() as u16);
        lines.push(Line::from(Span::styled(
            "[ Fix SQLITE_BUSY concurrency ]  [ Map architecture ]  [ Compare Hashline eval ]",
            Style::default().fg(t.ink_secondary),
        )));
        if chip_y < area.y.saturating_add(area.height) {
            let third = area.width / 3;
            hitmap.landing_prompts.push((
                Rect {
                    x: area.x,
                    y: chip_y,
                    width: third,
                    height: 1,
                },
                "Fix SQLITE_BUSY concurrency".to_string(),
            ));
            hitmap.landing_prompts.push((
                Rect {
                    x: area.x.saturating_add(third),
                    y: chip_y,
                    width: third,
                    height: 1,
                },
                "Map architecture".to_string(),
            ));
            hitmap.landing_prompts.push((
                Rect {
                    x: area.x.saturating_add(third * 2),
                    y: chip_y,
                    width: area.width.saturating_sub(third * 2),
                    height: 1,
                },
                "Compare Hashline eval".to_string(),
            ));
        }
    }
    frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, sess: &SessionSummaryDto) {
    let t = &app.theme;
    let mut lines: Vec<Line<'_>> = Vec::new();

    for item in &sess.messages {
        match item {
            TranscriptItemDto::User { text } => {
                for raw_line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!(" {raw_line} "),
                        Style::default().bg(t.user_bubble).fg(t.ink_primary),
                    )));
                }
                lines.push(Line::raw(""));
            }
            TranscriptItemDto::Handoff { text } => {
                lines.push(Line::from(Span::styled(
                    format!(" ── {text} ──"),
                    Style::default().fg(t.ink_tertiary),
                )));
                lines.push(Line::raw(""));
            }
            TranscriptItemDto::Activity {
                summary,
                working,
                expanded,
                tools,
            } => {
                let lead = if *working {
                    Span::styled(
                        format!(" {} ", app.spinner_glyph()),
                        Style::default().fg(t.status_working),
                    )
                } else {
                    Span::styled(
                        if *expanded { " ▾ " } else { " ▸ " },
                        Style::default().fg(t.ink_tertiary),
                    )
                };
                lines.push(Line::from(vec![
                    lead,
                    Span::styled(summary.as_str(), Style::default().fg(t.ink_secondary)),
                ]));
                if *expanded {
                    for tool in tools {
                        let arg_color = if tool.opens_diff {
                            t.text_accent
                        } else {
                            t.ink_secondary
                        };
                        lines.push(Line::from(vec![
                            Span::styled("   │ ", Style::default().fg(t.border)),
                            Span::styled(
                                format!("{:<8} ", tool.verb),
                                Style::default().fg(t.ink_primary),
                            ),
                            Span::styled(format!("{}  ", tool.arg), Style::default().fg(arg_color)),
                            Span::styled(tool.note.as_str(), Style::default().fg(t.ink_tertiary)),
                        ]));
                    }
                }
                lines.push(Line::raw(""));
            }
            TranscriptItemDto::Assistant { text, meta } => {
                for raw_line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!(" {raw_line}"),
                        Style::default().fg(t.ink_primary),
                    )));
                }
                if let Some(m) = meta {
                    lines.push(Line::from(Span::styled(
                        format!(" {m}  [Review diff]  [Fork here]"),
                        Style::default().fg(t.ink_tertiary),
                    )));
                }
                lines.push(Line::raw(""));
            }
        }
    }

    let width = area.width.max(1) as usize;
    let total_visual: u16 = lines
        .iter()
        .map(|l| {
            let w = l.width().max(1);
            w.div_ceil(width) as u16
        })
        .sum();
    let max_scroll = total_visual.saturating_sub(area.height);
    let scroll_y = max_scroll.saturating_sub(app.transcript_scroll.min(max_scroll));

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0)),
        area,
    );
}

fn render_decision_card(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    dec: &DecisionCardDto,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let right_title = if dec.kind == "ask" {
        " ask_user ".to_string()
    } else {
        format!(
            " ⛨ {} ▾ ",
            dec.sandbox.as_deref().unwrap_or("WorkspaceWrite")
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.panel).fg(t.ink_primary))
        .title(format!(" {} ", dec.title))
        .title_bottom(format!(" Keys 1-4 · Esc stop ·{right_title}"));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    hitmap.approval_sandbox_btn = Some(Rect {
        x: area.x.saturating_add(area.width.saturating_sub(24)),
        y: area.y.saturating_add(area.height.saturating_sub(1)),
        width: 24.min(area.width),
        height: 1,
    });

    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut row_off: u16 = 0;
    if dec.kind == "shell" {
        if let Some(ref cmd) = dec.command {
            lines.push(Line::from(Span::styled(
                format!(" $ {cmd}"),
                Style::default().bg(t.surface).fg(t.ink_primary),
            )));
            row_off = row_off.saturating_add(1);
        }
        if app.sandbox_details_open
            && let Some(ref info) = dec.sandbox_info
        {
            lines.push(Line::from(Span::styled(
                format!(" {info}"),
                Style::default().fg(t.ink_secondary),
            )));
            row_off = row_off.saturating_add(1);
        }
        let pat = dec.pattern.as_deref().unwrap_or("cargo *");
        // Choice numbers stay strictly neutral (`ink_secondary`), never colored (anti-slop invariant).
        let choices = [
            (1u8, "1", "Approve once", "Run this command"),
            (2u8, "2", "Always allow this session", pat),
            (3u8, "3", "Decline", "Let agent try another approach"),
            (4u8, "4", "Cancel turn", "Stop turn (UserDeclined)"),
        ];
        for (choice_idx, num, label, desc) in choices {
            if row_off < inner.height {
                hitmap.approval_choices.push((
                    Rect {
                        x: inner.x,
                        y: inner.y.saturating_add(row_off),
                        width: inner.width,
                        height: 1,
                    },
                    choice_idx,
                ));
            }
            lines.push(Line::from(vec![
                Span::styled(format!(" ({num}) "), Style::default().fg(t.ink_secondary)),
                Span::styled(format!("{label:<27}"), Style::default().fg(t.ink_primary)),
                Span::styled(desc, Style::default().fg(t.ink_tertiary)),
            ]));
            row_off = row_off.saturating_add(1);
        }
    } else {
        for (i, (label, desc)) in dec.options.iter().enumerate() {
            if row_off < inner.height {
                hitmap.approval_choices.push((
                    Rect {
                        x: inner.x,
                        y: inner.y.saturating_add(row_off),
                        width: inner.width,
                        height: 1,
                    },
                    (i + 1) as u8,
                ));
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" ({}) ", i + 1),
                    Style::default().fg(t.ink_secondary),
                ),
                Span::styled(format!("{label}  "), Style::default().fg(t.ink_primary)),
                Span::styled(desc.as_str(), Style::default().fg(t.ink_tertiary)),
            ]));
            row_off = row_off.saturating_add(1);
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_composer(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    sess: Option<&SessionSummaryDto>,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    hitmap.composer_area = Some(area);
    let border_color = if app.focus == crate::app::FocusTarget::Composer {
        t.accent
    } else {
        t.border_heavy
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(t.control_bg).fg(t.ink_primary));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 {
        return;
    }

    let input_line = if app.composer_input.is_empty() {
        Line::from(Span::styled(
            " Ask anything · / commands · @ files · $ skills",
            Style::default().fg(t.ink_tertiary),
        ))
    } else {
        Line::from(Span::styled(
            format!(" {}", app.composer_input),
            Style::default().fg(t.ink_primary),
        ))
    };

    let mode = sess.map(|s| s.interaction_mode).unwrap_or_default();
    let rmode = sess.map(|s| s.runtime_mode).unwrap_or_default();
    let ctx_label = sess.map(|s| s.ctx_label.as_str()).unwrap_or("0 / 110K");

    let mode_text = match mode {
        WireInteractionMode::Build => " Build  ",
        WireInteractionMode::Plan => " Plan  ",
    };
    let mode_span = match mode {
        WireInteractionMode::Build => Span::styled(mode_text, Style::default().fg(t.ink_secondary)),
        WireInteractionMode::Plan => {
            Span::styled(mode_text, Style::default().fg(t.status_working_ink))
        }
    };
    let access_text = rmode.display_pill();
    let access_span = match rmode {
        WireRuntimeMode::ApprovalRequired => {
            Span::styled(access_text, Style::default().fg(t.ink_secondary))
        }
        WireRuntimeMode::Auto => Span::styled(access_text, Style::default().fg(t.status_done_ink)),
        WireRuntimeMode::FullAccess => {
            Span::styled(access_text, Style::default().fg(t.full_access_ink))
        }
    };

    // Footer contains ONLY Build/Plan, runtimeAccess, contextUsage, send (NO /btw or /tree buttons).
    let foot_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(20), Constraint::Length(22)])
        .split(Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        });

    let mode_w = mode_text.chars().count() as u16;
    let access_w = access_text.chars().count() as u16;
    hitmap.composer_mode_btn = Some(Rect {
        x: foot_cols[0].x,
        y: foot_cols[0].y,
        width: mode_w.min(foot_cols[0].width),
        height: 1,
    });
    hitmap.composer_access_btn = Some(Rect {
        x: foot_cols[0].x.saturating_add(mode_w),
        y: foot_cols[0].y,
        width: access_w.min(foot_cols[0].width.saturating_sub(mode_w)),
        height: 1,
    });
    hitmap.composer_send_btn = Some(Rect {
        x: foot_cols[1]
            .x
            .saturating_add(foot_cols[1].width.saturating_sub(4)),
        y: foot_cols[1].y,
        width: 4.min(foot_cols[1].width),
        height: 1,
    });

    frame.render_widget(
        Paragraph::new(input_line),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![mode_span, access_span])),
        foot_cols[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{ctx_label}  "),
                Style::default().fg(t.ink_tertiary),
            ),
            Span::styled("[↑]", Style::default().bg(t.ink_primary).fg(t.surface)),
        ]))
        .alignment(Alignment::Right),
        foot_cols[1],
    );

    let overlay_blocking_cursor = app.btw_open
        || app.tree_modal_open
        || app.model_popover_open
        || app.access_popover_open
        || app.cache_popover_open
        || app.board_open;
    if !overlay_blocking_cursor && inner.width > 2 {
        let byte_end = app.composer_cursor.min(app.composer_input.len());
        let mut safe_end = byte_end;
        while safe_end > 0 && !app.composer_input.is_char_boundary(safe_end) {
            safe_end -= 1;
        }
        let col_offset = app.composer_input[..safe_end].chars().count() as u16;
        let cursor_x = inner
            .x
            .saturating_add(1)
            .saturating_add(col_offset)
            .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
        frame.set_cursor_position(Position::new(cursor_x, inner.y));
    }
}

fn render_board(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &TuiApp,
    spec: &ResponsiveLayoutSpec,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(6)])
        .split(area);

    hitmap.overlay_hits.push((rows[0], OverlayHit::BoardBack));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                " Agent Board   [Back to session · Esc]",
                Style::default()
                    .fg(t.ink_primary)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                " Every session on this daemon. Use arrows + Enter or click a card to jump in.",
                Style::default().fg(t.ink_tertiary),
            )),
        ]),
        rows[0],
    );

    type BoardColDef<'a> = (&'a str, fn(&SessionSummaryDto) -> bool, &'a str);
    let col_defs: [BoardColDef<'_>; 3] = [
        (
            "Needs you",
            |s| s.status == SessionUiStatus::Blocked,
            "Nothing is waiting on you. Approvals and agent questions land here.",
        ),
        (
            "Working",
            |s| s.status == SessionUiStatus::Working,
            "No session is running. Send a prompt to start one.",
        ),
        (
            "Ready / Idle",
            |s| matches!(s.status, SessionUiStatus::Done | SessionUiStatus::Idle),
            "Finished sessions show up here with their diff.",
        ),
    ];

    let visible_cols = spec.board_columns.clamp(1, 3);
    let constraints: Vec<Constraint> = (0..visible_cols)
        .map(|_| Constraint::Ratio(1, visible_cols as u32))
        .collect();
    let grid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(rows[1]);

    let selected_id = app
        .board_session_ids()
        .get(app.board_selected_idx)
        .cloned()
        .unwrap_or_default();

    for (idx, &(title, pred, empty_msg)) in col_defs.iter().take(visible_cols).enumerate() {
        let items: Vec<&SessionSummaryDto> = app.sessions.iter().filter(|s| pred(s)).collect();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.border))
            .style(Style::default().bg(t.sidebar_bg).fg(t.ink_primary))
            .title(format!(" {title} ({}) ", items.len()));
        let inner = block.inner(grid[idx]);
        frame.render_widget(block, grid[idx]);

        let mut lines = Vec::new();
        if items.is_empty() {
            lines.push(Line::from(Span::styled(
                empty_msg,
                Style::default().fg(t.ink_secondary),
            )));
        } else {
            let mut row_off: u16 = 0;
            for s in items {
                let is_sel = s.id == selected_id;
                let bg = if is_sel { t.user_bubble } else { t.sidebar_bg };
                let mark = if is_sel { " ✓" } else { "" };
                if row_off < inner.height {
                    hitmap.overlay_hits.push((
                        Rect {
                            x: inner.x,
                            y: inner.y.saturating_add(row_off),
                            width: inner.width,
                            height: 2.min(inner.height.saturating_sub(row_off)),
                        },
                        OverlayHit::BoardSession(s.id.clone()),
                    ));
                }
                lines.push(Line::from(Span::styled(
                    format!("{}{mark}", s.name),
                    Style::default()
                        .fg(t.ink_primary)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(Span::styled(
                    format!("{} · {}", s.model_label, s.ctx_label),
                    Style::default().fg(t.ink_tertiary).bg(bg),
                )));
                lines.push(Line::raw(""));
                row_off = row_off.saturating_add(3);
            }
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

fn render_diff_drawer(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let sess = app.current_session();
    let title = match sess.and_then(|s| s.diff.as_ref()) {
        Some(d) => format!(" Changes +{} -{}  [#hash] ", d.added, d.removed),
        None => " Changes ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface).fg(t.ink_primary))
        .title(title)
        .title_bottom(" ✓ tree-sitter 0.4ms · LSP 0 err ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    hitmap.diff_hashline_btn = Some(Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    });

    let mut lines = Vec::new();
    if sess.is_none_or(|s| s.diff.is_none()) {
        lines.push(Line::from(Span::styled(
            " No changes in this session yet",
            Style::default()
                .fg(t.ink_primary)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            " Edits show up here after the agent's first edit call.",
            Style::default().fg(t.ink_secondary),
        )));
    } else if let Some((file_name, diff_lines)) = app.diff_files.get(app.diff_active_file) {
        lines.push(Line::from(Span::styled(
            format!(" {file_name}"),
            Style::default().bg(t.user_bubble).fg(t.ink_primary),
        )));
        for row in diff_lines {
            let (bg, sign_fg) = match row.sign {
                '+' => (t.diff_added_bg, t.status_done_ink),
                '-' => (t.diff_removed_bg, t.status_error_ink),
                _ => (t.surface, t.ink_tertiary),
            };
            let ln = if app.diff_show_hashline {
                row.hashline.as_str()
            } else {
                row.line_num.as_str()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {ln} "),
                    Style::default().fg(t.ink_secondary).bg(bg),
                ),
                Span::styled(
                    format!("{} ", row.sign),
                    Style::default().fg(sign_fg).bg(bg),
                ),
                Span::styled(row.code.as_str(), Style::default().fg(t.ink_primary).bg(bg)),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_cli_drawer(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface_under).fg(t.ink_secondary))
        .title(" Terminal · saint-harness CLI  [server status | eval --repeat 3 | models] ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    hitmap.overlay_hits.push((
        Rect {
            x: area.x.saturating_add(31),
            y: area.y,
            width: 15,
            height: 1,
        },
        OverlayHit::CliTab(CliTab::Status),
    ));
    hitmap.overlay_hits.push((
        Rect {
            x: area.x.saturating_add(47),
            y: area.y,
            width: 17,
            height: 1,
        },
        OverlayHit::CliTab(CliTab::Eval),
    ));
    hitmap.overlay_hits.push((
        Rect {
            x: area.x.saturating_add(65),
            y: area.y,
            width: 9,
            height: 1,
        },
        OverlayHit::CliTab(CliTab::Models),
    ));

    let body = if app.cli_tab == CliTab::Status && app.daemon_state == DaemonConnectionState::Error
    {
        if app.cli_json_mode {
            format!(
                "$ saint-harness server status --json\n{{\"reachable\":false,\"ready\":false,\"error\":\"connection refused\",\"url\":\"{}\"}}",
                app.daemon_url
            )
        } else {
            format!(
                "$ saint-harness server status\nerror: SaintHarness server is not reachable at {}\nhint: start it with `saint-harness server`, or check $XDG_RUNTIME_DIR/saint-harness.sock",
                app.daemon_url
            )
        }
    } else {
        match app.cli_tab {
            CliTab::Status => format!(
                "$ saint-harness server status\nSaintHarness server: ready ({})\nProjection: healthy (SQLite WAL · {} sessions)",
                app.daemon_url,
                app.sessions.len()
            ),
            CliTab::Eval => "$ saint-harness eval benchmarks/core.yaml --repeat 3\n# sample values to show the layout; no eval has been run yet\nqwen-3.8-27b      hashline     83.3%    89.2%".to_string(),
            CliTab::Models => "$ saint-harness models\nqwen-3.8-27b     exact:qwen3.8-27b    110K / 262K  hashline\nclaude-flagship  latest_flagship      auto (ETag)  str_replace\ngemini-flagship  latest_flagship      auto (ETag)  hashline".to_string(),
        }
    };
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
}

fn render_cache_popover(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let w = 38u16.min(area.width.saturating_sub(2));
    let h = 10u16.min(area.height.saturating_sub(2));
    let pop = Rect {
        x: area.x + area.width.saturating_sub(w + 1),
        y: area.y + 1,
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(" Prompt cache ");
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    let s = app.current_session();
    let state_label = if s.is_some_and(|x| x.cache_warm) {
        Span::styled("Warm", Style::default().fg(t.status_done_ink))
    } else {
        Span::styled("Cold", Style::default().fg(t.status_blocked_ink))
    };
    let lines = vec![
        Line::from(vec![Span::raw(" State: "), state_label]),
        Line::from(format!(
            " PruneEpoch: #{} frozen ({} masks queued)",
            s.map(|x| x.prune_epoch).unwrap_or(0),
            s.map(|x| x.queued_masks).unwrap_or(0)
        )),
        Line::from(format!(
            " Session cost: ${:.3}",
            s.map(|x| x.cost_usd).unwrap_or(0.0)
        )),
        Line::from(Span::styled(
            " Sample values. Nothing here was measured.",
            Style::default().fg(t.ink_tertiary),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_model_popover(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let w = 68u16.min(area.width.saturating_sub(2));
    let h = 18u16.min(area.height.saturating_sub(2));
    let pop = Rect {
        x: area.x + area.width.saturating_sub(w + 1),
        y: area.y + 1,
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(" Cross-model handoff (↑↓ move · Enter select · Esc close) ");
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    let current_key = app
        .current_session()
        .map(|s| s.model_key.as_str())
        .unwrap_or("");
    let rows = app.model_picker_rows();
    let focused_row = rows.get(app.model_picker_idx);

    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut row_idx = 0usize;
    let mut y_off = 0u16;

    for leaf in &app.model_leaves {
        let is_cur = leaf.key == current_key;
        let is_focus = matches!(focused_row, Some(ModelPickerRow::Leaf(k)) if k == &leaf.key);
        let bg = if is_focus || is_cur {
            t.user_bubble
        } else {
            t.elevated_bg
        };
        let mark = if is_cur { " ✓" } else { "" };
        if y_off < inner.height {
            hitmap.overlay_hits.push((
                Rect {
                    x: inner.x,
                    y: inner.y.saturating_add(y_off),
                    width: inner.width,
                    height: 1,
                },
                OverlayHit::ModelRow(row_idx),
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {}{mark}  ", leaf.label),
                Style::default().fg(t.ink_primary).bg(bg),
            ),
            Span::styled(
                leaf.tag.as_str(),
                Style::default().fg(t.ink_tertiary).bg(bg),
            ),
        ]));
        row_idx += 1;
        y_off = y_off.saturating_add(1);
    }
    lines.push(Line::from(Span::styled(
        " ──────────────────────────────────────────",
        Style::default().fg(t.divider_hairline),
    )));
    y_off = y_off.saturating_add(1);

    for prov in &app.model_providers {
        let open = app.expanded_providers.contains(&prov.key);
        let holds_cur = prov.children.iter().any(|c| c.key == current_key);
        let is_focus = matches!(focused_row, Some(ModelPickerRow::Provider(k)) if k == &prov.key);
        let bg = if is_focus {
            t.user_bubble
        } else {
            t.elevated_bg
        };
        let caret = if open { "▼" } else { "▶" };
        let mark = if holds_cur && !open { " ✓" } else { "" };
        if y_off < inner.height {
            hitmap.overlay_hits.push((
                Rect {
                    x: inner.x,
                    y: inner.y.saturating_add(y_off),
                    width: inner.width,
                    height: 1,
                },
                OverlayHit::ModelRow(row_idx),
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {caret} "),
                Style::default().fg(t.ink_tertiary).bg(bg),
            ),
            Span::styled(
                format!("{}{mark}  ", prov.label),
                Style::default().fg(t.ink_primary).bg(bg),
            ),
            Span::styled(
                prov.tag.as_str(),
                Style::default().fg(t.ink_tertiary).bg(bg),
            ),
        ]));
        row_idx += 1;
        y_off = y_off.saturating_add(1);

        if !open {
            continue;
        }
        let state = app
            .provider_catalog_state
            .get(&prov.key)
            .copied()
            .unwrap_or(CatalogLoadState::Loaded);
        match state {
            CatalogLoadState::Error => {
                lines.push(Line::from(Span::styled(
                    format!(
                        " │ Can't load the {} catalog: the daemon is offline. [Retry]",
                        prov.catalog
                    ),
                    Style::default().fg(t.status_error_ink),
                )));
                y_off = y_off.saturating_add(1);
            }
            CatalogLoadState::Loading => {
                lines.push(Line::from(Span::styled(
                    format!(
                        " │ Loading {} models from the {} catalog…",
                        prov.label, prov.catalog
                    ),
                    Style::default().fg(t.ink_secondary),
                )));
                y_off = y_off.saturating_add(1);
            }
            CatalogLoadState::Loaded | CatalogLoadState::Idle => {
                for child in &prov.children {
                    let is_cur = child.key == current_key;
                    let is_focus = matches!(
                        focused_row,
                        Some(ModelPickerRow::Child { key, .. }) if key == &child.key
                    );
                    let cbg = if is_focus || is_cur {
                        t.user_bubble
                    } else {
                        t.elevated_bg
                    };
                    let mark = if is_cur { " ✓" } else { "" };
                    if y_off < inner.height {
                        hitmap.overlay_hits.push((
                            Rect {
                                x: inner.x,
                                y: inner.y.saturating_add(y_off),
                                width: inner.width,
                                height: 1,
                            },
                            OverlayHit::ModelRow(row_idx),
                        ));
                    }
                    lines.push(Line::from(vec![
                        Span::styled(" │ ", Style::default().fg(t.border)),
                        Span::styled(
                            format!("{}{mark}  ", child.label),
                            Style::default().fg(t.ink_primary).bg(cbg),
                        ),
                        Span::styled(
                            child.selector.as_str(),
                            Style::default().fg(t.ink_tertiary).bg(cbg),
                        ),
                    ]));
                    row_idx += 1;
                    y_off = y_off.saturating_add(1);
                }
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_access_popover(
    frame: &mut Frame<'_>,
    main_area: Rect,
    app: &TuiApp,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let w = 52u16.min(main_area.width.saturating_sub(2));
    let h = 7u16.min(main_area.height.saturating_sub(4));
    let pop = Rect {
        x: main_area.x + 1,
        y: main_area.y + main_area.height.saturating_sub(h + 4),
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(" Runtime mode (↑↓ select · Enter apply · Esc) ");
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    let current = app
        .current_session()
        .map(|s| s.runtime_mode)
        .unwrap_or_default();
    let rows = [
        (
            WireRuntimeMode::ApprovalRequired,
            "○ Ask for approval",
            "approval-required",
            t.ink_tertiary,
        ),
        (
            WireRuntimeMode::Auto,
            "● Auto mode",
            "--auto",
            t.ink_tertiary,
        ),
        (
            WireRuntimeMode::FullAccess,
            "▲ Bypass permissions",
            "full-access",
            t.full_access_ink,
        ),
    ];
    let mut lines = Vec::new();
    for (idx, (mode, label, tag, tag_color)) in rows.into_iter().enumerate() {
        let active = current == mode;
        let focused = idx == app.access_popover_idx;
        let bg = if focused || active {
            t.user_bubble
        } else {
            t.elevated_bg
        };
        let mark = if active { "  ✓" } else { "" };
        if (idx as u16) < inner.height {
            hitmap.overlay_hits.push((
                Rect {
                    x: inner.x,
                    y: inner.y.saturating_add(idx as u16),
                    width: inner.width,
                    height: 1,
                },
                OverlayHit::AccessMode(mode),
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {label}{mark}  "),
                Style::default().fg(t.ink_primary).bg(bg),
            ),
            Span::styled(tag, Style::default().fg(tag_color).bg(bg)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_inline_palette(
    frame: &mut Frame<'_>,
    main_area: Rect,
    app: &TuiApp,
    hitmap: &mut UiHitMap,
) {
    let t = &app.theme;
    let w = main_area.width.saturating_sub(4).min(68);
    let h = 10u16.min(main_area.height.saturating_sub(5));
    let pop = Rect {
        x: main_area.x + 2,
        y: main_area.y + main_area.height.saturating_sub(h + 4),
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let title = match app.inline_palette_trigger {
        Some('/') => " Slash commands  (↑↓ select · Enter run · Esc close) ",
        Some('@') => " Files (nucleo-matcher) ",
        Some('$') => " Skills & MCP ",
        _ => " Suggestions ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(title);
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    let mut lines = Vec::new();
    if app.inline_palette_items.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                " No match for \"{}\". Esc to close, or keep typing to send it as a message.",
                app.composer_input
            ),
            Style::default().fg(t.ink_secondary),
        )));
    } else {
        for (idx, item) in app.inline_palette_items.iter().enumerate() {
            let active = idx == app.inline_palette_idx;
            let bg = if active { t.user_bubble } else { t.elevated_bg };
            if (idx as u16) < inner.height {
                hitmap.overlay_hits.push((
                    Rect {
                        x: inner.x,
                        y: inner.y.saturating_add(idx as u16),
                        width: inner.width,
                        height: 1,
                    },
                    OverlayHit::PaletteRow(idx),
                ));
            }
            // Leading key stays neutral ink_primary (anti-slop: no colored leading text in menus).
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:<14} ", item.key),
                    Style::default()
                        .fg(t.ink_primary)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    item.description.as_str(),
                    Style::default().fg(t.ink_tertiary).bg(bg),
                ),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_btw_sheet(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let w = 52u16.min(area.width.saturating_sub(2));
    let h = 9u16.min(area.height.saturating_sub(2));
    let pop = Rect {
        x: area.x + area.width.saturating_sub(w + 1),
        y: area.y + area.height.saturating_sub(h + 1),
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(" /btw · warm prefix, no history writes  [Esc] ");
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    let input_y = inner.y.saturating_add(inner.height.saturating_sub(1));
    let out_h = inner.height.saturating_sub(1);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            app.btw_output.as_str(),
            Style::default().fg(t.ink_secondary),
        )))
        .wrap(Wrap { trim: false }),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: out_h,
        },
    );
    let prompt_prefix = " Side question: ";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{prompt_prefix}{}", app.btw_input),
            Style::default().fg(t.ink_primary),
        ))),
        Rect {
            x: inner.x,
            y: input_y,
            width: inner.width,
            height: 1,
        },
    );
    let cursor_x = inner
        .x
        .saturating_add((prompt_prefix.chars().count() + app.btw_input.chars().count()) as u16)
        .min(inner.x.saturating_add(inner.width.saturating_sub(1)));
    frame.set_cursor_position(Position::new(cursor_x, input_y));
}

fn render_tree_modal(frame: &mut Frame<'_>, area: Rect, app: &TuiApp, hitmap: &mut UiHitMap) {
    let t = &app.theme;
    let w = 58u16.min(area.width.saturating_sub(4));
    let h = 11u16.min(area.height.saturating_sub(4));
    let pop = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    hitmap.overlay_rect = Some(pop);
    frame.render_widget(Clear, pop);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_heavy))
        .style(Style::default().bg(t.elevated_bg).fg(t.ink_primary))
        .title(" Session tree  [f: Fork HEAD]  [Esc] ")
        .title_bottom(" ↑↓ move · Enter switch HEAD · Walks parent_msg_id ");
    let inner = block.inner(pop);
    frame.render_widget(block, pop);

    hitmap.overlay_hits.push((
        Rect {
            x: pop.x.saturating_add(15),
            y: pop.y,
            width: 15,
            height: 1,
        },
        OverlayHit::TreeForkHead,
    ));

    let mut lines = Vec::new();
    for (idx, node) in app.tree_nodes.iter().enumerate() {
        let is_head = node.id == app.tree_head_id;
        let is_sel = idx == app.tree_selected_idx;
        let bg = if is_sel || is_head {
            t.user_bubble
        } else {
            t.elevated_bg
        };
        let badge = if is_head { "  HEAD ✓" } else { "" };
        if (idx as u16) < inner.height {
            hitmap.overlay_hits.push((
                Rect {
                    x: inner.x,
                    y: inner.y.saturating_add(idx as u16),
                    width: inner.width,
                    height: 1,
                },
                OverlayHit::TreeNode(idx),
            ));
        }
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {}", node.prefix),
                Style::default().fg(t.ink_tertiary).bg(bg),
            ),
            Span::styled(
                format!("{} ", node.id),
                Style::default()
                    .fg(t.ink_primary)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}{badge}", node.label),
                Style::default().fg(t.ink_secondary).bg(bg),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
