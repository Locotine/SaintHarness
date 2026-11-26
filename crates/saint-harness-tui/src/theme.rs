use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    SaintDark,
    SaintLight,
    Ansi16,
    NoColor,
}

impl ThemeMode {
    pub fn parse(raw: &str) -> Self {
        match raw.to_ascii_lowercase().as_str() {
            "saint-light" | "light" => Self::SaintLight,
            "ansi16" => Self::Ansi16,
            "no-color" | "none" => Self::NoColor,
            _ => Self::SaintDark,
        }
    }
}

/// Centralized Saint Hyper Agent quiet-chrome theme tokens.
/// Status hues are fills only; all colored text must use the `*_ink` fields to clear WCAG AA >= 4.5:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaintTheme {
    pub mode: ThemeMode,
    pub surface: Color,
    pub surface_under: Color,
    pub sidebar_bg: Color,
    pub panel: Color,
    pub control_bg: Color,
    pub elevated_bg: Color,
    pub user_bubble: Color,
    pub ink_primary: Color,
    pub ink_secondary: Color,
    pub ink_tertiary: Color,
    pub border: Color,
    pub border_heavy: Color,
    pub divider_hairline: Color,
    pub accent: Color,
    pub text_accent: Color,
    pub accent_bg: Color,
    pub full_access: Color,
    pub full_access_ink: Color,
    pub status_done: Color,
    pub status_done_ink: Color,
    pub status_blocked: Color,
    pub status_blocked_ink: Color,
    pub status_working: Color,
    pub status_working_ink: Color,
    pub status_error: Color,
    pub status_error_ink: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
}

impl SaintTheme {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::SaintDark => Self::dark(),
            ThemeMode::SaintLight => Self::light(),
            ThemeMode::Ansi16 => Self::ansi16(),
            ThemeMode::NoColor => Self::no_color(),
        }
    }

    pub const fn dark() -> Self {
        Self {
            mode: ThemeMode::SaintDark,
            surface: Color::Rgb(17, 17, 17),
            surface_under: Color::Rgb(12, 12, 12),
            sidebar_bg: Color::Rgb(20, 20, 20),
            panel: Color::Rgb(23, 23, 23),
            control_bg: Color::Rgb(28, 28, 28),
            elevated_bg: Color::Rgb(32, 32, 32),
            user_bubble: Color::Rgb(28, 28, 28),
            ink_primary: Color::Rgb(252, 252, 252),
            ink_secondary: Color::Rgb(163, 163, 163),
            ink_tertiary: Color::Rgb(154, 154, 154),
            border: Color::Rgb(38, 38, 38),
            border_heavy: Color::Rgb(52, 52, 52),
            divider_hairline: Color::Rgb(30, 30, 30),
            accent: Color::Rgb(242, 97, 45),
            text_accent: Color::Rgb(246, 144, 108),
            accent_bg: Color::Rgb(44, 27, 20),
            full_access: Color::Rgb(254, 133, 73),
            full_access_ink: Color::Rgb(254, 133, 73),
            status_done: Color::Rgb(0, 162, 64),
            status_done_ink: Color::Rgb(34, 180, 95),
            status_blocked: Color::Rgb(245, 180, 74),
            status_blocked_ink: Color::Rgb(245, 180, 74),
            status_working: Color::Rgb(176, 109, 255),
            status_working_ink: Color::Rgb(192, 140, 255),
            status_error: Color::Rgb(224, 46, 42),
            status_error_ink: Color::Rgb(239, 99, 96),
            diff_added_bg: Color::Rgb(14, 46, 26),
            diff_removed_bg: Color::Rgb(58, 23, 22),
        }
    }

    pub const fn light() -> Self {
        Self {
            mode: ThemeMode::SaintLight,
            surface: Color::Rgb(255, 255, 255),
            surface_under: Color::Rgb(246, 246, 246),
            sidebar_bg: Color::Rgb(250, 250, 250),
            panel: Color::Rgb(248, 248, 248),
            control_bg: Color::Rgb(243, 243, 243),
            elevated_bg: Color::Rgb(255, 255, 255),
            user_bubble: Color::Rgb(245, 245, 245),
            ink_primary: Color::Rgb(13, 13, 13),
            ink_secondary: Color::Rgb(105, 105, 105),
            ink_tertiary: Color::Rgb(104, 104, 104),
            border: Color::Rgb(234, 234, 234),
            border_heavy: Color::Rgb(221, 221, 221),
            divider_hairline: Color::Rgb(239, 239, 239),
            accent: Color::Rgb(199, 70, 20),
            text_accent: Color::Rgb(184, 63, 16),
            accent_bg: Color::Rgb(250, 238, 234),
            full_access: Color::Rgb(226, 85, 5),
            full_access_ink: Color::Rgb(163, 58, 3),
            status_done: Color::Rgb(0, 162, 64),
            status_done_ink: Color::Rgb(4, 108, 78),
            status_blocked: Color::Rgb(217, 119, 6),
            status_blocked_ink: Color::Rgb(180, 83, 9),
            status_working: Color::Rgb(117, 30, 217),
            status_working_ink: Color::Rgb(117, 30, 217),
            status_error: Color::Rgb(224, 46, 42),
            status_error_ink: Color::Rgb(198, 37, 33),
            diff_added_bg: Color::Rgb(224, 244, 232),
            diff_removed_bg: Color::Rgb(252, 232, 232),
        }
    }

    pub const fn ansi16() -> Self {
        Self {
            mode: ThemeMode::Ansi16,
            surface: Color::Reset,
            surface_under: Color::Black,
            sidebar_bg: Color::Reset,
            panel: Color::Black,
            control_bg: Color::Black,
            elevated_bg: Color::Black,
            user_bubble: Color::DarkGray,
            ink_primary: Color::White,
            ink_secondary: Color::Gray,
            ink_tertiary: Color::Gray,
            border: Color::DarkGray,
            border_heavy: Color::Gray,
            divider_hairline: Color::DarkGray,
            accent: Color::Red,
            text_accent: Color::LightRed,
            accent_bg: Color::Black,
            full_access: Color::LightRed,
            full_access_ink: Color::LightRed,
            status_done: Color::Green,
            status_done_ink: Color::LightGreen,
            status_blocked: Color::Yellow,
            status_blocked_ink: Color::LightYellow,
            status_working: Color::Magenta,
            status_working_ink: Color::LightMagenta,
            status_error: Color::Red,
            status_error_ink: Color::LightRed,
            diff_added_bg: Color::Black,
            diff_removed_bg: Color::Black,
        }
    }

    pub const fn no_color() -> Self {
        Self {
            mode: ThemeMode::NoColor,
            surface: Color::Reset,
            surface_under: Color::Reset,
            sidebar_bg: Color::Reset,
            panel: Color::Reset,
            control_bg: Color::Reset,
            elevated_bg: Color::Reset,
            user_bubble: Color::Reset,
            ink_primary: Color::Reset,
            ink_secondary: Color::Reset,
            ink_tertiary: Color::Reset,
            border: Color::Reset,
            border_heavy: Color::Reset,
            divider_hairline: Color::Reset,
            accent: Color::Reset,
            text_accent: Color::Reset,
            accent_bg: Color::Reset,
            full_access: Color::Reset,
            full_access_ink: Color::Reset,
            status_done: Color::Reset,
            status_done_ink: Color::Reset,
            status_blocked: Color::Reset,
            status_blocked_ink: Color::Reset,
            status_working: Color::Reset,
            status_working_ink: Color::Reset,
            status_error: Color::Reset,
            status_error_ink: Color::Reset,
            diff_added_bg: Color::Reset,
            diff_removed_bg: Color::Reset,
        }
    }

    pub fn token_rgb(color: Color) -> Option<[u8; 3]> {
        match color {
            Color::Rgb(r, g, b) => Some([r, g, b]),
            _ => None,
        }
    }
}

pub fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let lin = |c: u8| {
        let s = f64::from(c) / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
}

pub fn contrast_ratio(fg: [u8; 3], bg: [u8; 3]) -> f64 {
    let l1 = relative_luminance(fg);
    let l2 = relative_luminance(bg);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}
