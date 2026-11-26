pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const SPINNER_INTERVAL_MS: u64 = 65;
pub const LANDING_WAVE_SETTLES_AFTER_MS: u64 = 5_000;

pub const BRAILLE_STANDARD_COLS: usize = 16;
pub const BRAILLE_STANDARD_ROWS: usize = 13;
pub const BRAILLE_STANDARD_MIN_TERMINAL_ROWS: u16 = 28;
pub const BRAILLE_STANDARD_CROWN_ROWS: [usize; 3] = [0, 1, 2];

pub const BRAILLE_STANDARD_LINES: [&str; 13] = [
    "     ⡀ ⣰⣆ ⢀",
    "  ⠈⢷⣤⣿⠶⠿⠿⠶⣿⣤⡾⠁",
    "   ⠈⠳⢂⣠⣤⣤⣄⡐⠞⠁",
    "⢀⣠⣴⣶⣿⣿⣿⡿⢿⣿⣿⣿⣶⣦⣄⡀",
    "⣿⡟⠉⣱⣿⢋⣴⡶⢶⣦⡝⢿⣎⠉⢻⣿",
    "⣿⡇ ⣿⡇⣿⣏  ⢹⣿⣼⣿ ⢸⣿",
    "⣿⡇ ⠹⣿⣌⠻⢷⢆⣄⡈⠙⠿ ⢸⣿",
    "⣿⡇ ⡀⠈⠛⠿⢂⣬⡛⢿⣦⡀ ⢸⣿",
    "⣿⣇ ⣿⣷⣦⡀⠈⠙⢿⣧⢹⣷ ⣸⣿",
    "⠹⣿⡄⢻⣧⠻⣷⣤⣤⣾⠏⣼⡟⢠⣿⠏",
    " ⠹⣿⣆⠻⢷⣦⡍⢩⣴⡾⠟⣰⣿⠏",
    "  ⠈⠻⣿⣦⣈⠁⠈⣁⣴⣿⠟⠁",
    "    ⠈⠙⠻⢿⡿⠟⠋⠁",
];

pub const BRAILLE_COMPACT_COLS: usize = 10;
pub const BRAILLE_COMPACT_ROWS: usize = 8;
pub const BRAILLE_COMPACT_MIN_TERMINAL_ROWS: u16 = 20;
pub const BRAILLE_COMPACT_CROWN_ROWS: [usize; 2] = [0, 1];

pub const BRAILLE_COMPACT_LINES: [&str; 8] = [
    "  ⣀⣄⣰⣆⣠⣀",
    "  ⠛⣉⣥⣬⣩⠛",
    "⣴⠞⣻⢟⣿⣿⡻⣟⠳⣦",
    "⣿ ⣿⢿⣅⠈⠿⣿ ⣿",
    "⣿ ⡘⠳⢏⣝⢶⣌ ⣿",
    "⢿⡀⣿⢷⣀⣉⡷⣿⢀⡿",
    "⠈⢷⣜⠷⡭⢭⠾⣣⡾⠁",
    "  ⠙⠻⢦⡴⠞⠋",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoVariant {
    BrailleStandard,
    BrailleCompact,
}

impl LogoVariant {
    pub fn as_contract_name(self) -> &'static str {
        match self {
            LogoVariant::BrailleStandard => "brailleStandard",
            LogoVariant::BrailleCompact => "brailleCompact",
        }
    }

    pub fn lines(self) -> &'static [&'static str] {
        match self {
            LogoVariant::BrailleStandard => &BRAILLE_STANDARD_LINES,
            LogoVariant::BrailleCompact => &BRAILLE_COMPACT_LINES,
        }
    }

    pub fn is_crown_row(self, row: usize) -> bool {
        match self {
            LogoVariant::BrailleStandard => BRAILLE_STANDARD_CROWN_ROWS.contains(&row),
            LogoVariant::BrailleCompact => BRAILLE_COMPACT_CROWN_ROWS.contains(&row),
        }
    }
}
