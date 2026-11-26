use crate::brand::{BRAILLE_STANDARD_MIN_TERMINAL_ROWS, LogoVariant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffDrawerMode {
    Overlay,
    Split,
}

impl DiffDrawerMode {
    pub fn as_contract_name(self) -> &'static str {
        match self {
            DiffDrawerMode::Overlay => "overlay",
            DiffDrawerMode::Split => "split",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsiveLayoutSpec {
    pub breakpoint_name: &'static str,
    pub sidebar_default_open: bool,
    pub sidebar_cols: u16,
    pub diff_drawer_mode: DiffDrawerMode,
    pub diff_drawer_cols: u16,
    pub board_columns: usize,
    pub logo_variant: LogoVariant,
    pub max_header_glance_items: usize,
}

/// Resolves exact responsive layout geometry matching the 5 TUI breakpoints in `tui-parity-contract.json`:
/// `ultra_compact` (80x24), `narrow` (100x30), `standard` (120x38), `split_review` (140x42), `wide` (180x50).
pub fn resolve_layout(cols: u16, rows: u16) -> ResponsiveLayoutSpec {
    let logo_for_height = |preferred: LogoVariant| {
        if rows < BRAILLE_STANDARD_MIN_TERMINAL_ROWS {
            LogoVariant::BrailleCompact
        } else {
            preferred
        }
    };

    if cols < 100 {
        ResponsiveLayoutSpec {
            breakpoint_name: "ultra_compact",
            sidebar_default_open: false,
            sidebar_cols: 26,
            diff_drawer_mode: DiffDrawerMode::Overlay,
            diff_drawer_cols: cols.saturating_sub(4).max(32),
            board_columns: 1,
            logo_variant: LogoVariant::BrailleCompact,
            max_header_glance_items: 2,
        }
    } else if cols < 120 {
        ResponsiveLayoutSpec {
            breakpoint_name: "narrow",
            sidebar_default_open: false,
            sidebar_cols: 28,
            diff_drawer_mode: DiffDrawerMode::Overlay,
            diff_drawer_cols: cols.saturating_sub(8).max(40),
            board_columns: 2,
            logo_variant: logo_for_height(LogoVariant::BrailleStandard),
            max_header_glance_items: 3,
        }
    } else if cols < 140 {
        ResponsiveLayoutSpec {
            breakpoint_name: "standard",
            sidebar_default_open: true,
            sidebar_cols: 28,
            diff_drawer_mode: DiffDrawerMode::Split,
            diff_drawer_cols: 44,
            board_columns: 3,
            logo_variant: logo_for_height(LogoVariant::BrailleStandard),
            max_header_glance_items: 3,
        }
    } else if cols < 180 {
        ResponsiveLayoutSpec {
            breakpoint_name: "split_review",
            sidebar_default_open: true,
            sidebar_cols: 28,
            diff_drawer_mode: DiffDrawerMode::Split,
            diff_drawer_cols: 54,
            board_columns: 3,
            logo_variant: logo_for_height(LogoVariant::BrailleStandard),
            max_header_glance_items: 3,
        }
    } else {
        ResponsiveLayoutSpec {
            breakpoint_name: "wide",
            sidebar_default_open: true,
            sidebar_cols: 30,
            diff_drawer_mode: DiffDrawerMode::Split,
            diff_drawer_cols: 68,
            board_columns: 3,
            logo_variant: logo_for_height(LogoVariant::BrailleStandard),
            max_header_glance_items: 3,
        }
    }
}
