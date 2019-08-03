use colored::Color;

const COLORS: &[Color] = &[
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::White,
    Color::BrightBlack,
    Color::BrightRed,
    Color::BrightGreen,
    Color::BrightYellow,
    Color::BrightBlue,
    Color::BrightMagenta,
    Color::BrightCyan,
    Color::BrightWhite,
];

pub struct Colorizer {
    idx: usize,
}

impl Colorizer {
    pub fn new() -> Self {
        Self { idx: 0 }
    }

    pub fn next(&mut self) -> Color {
        self.idx = (self.idx + 1) % COLORS.len();
        COLORS[self.idx]
    }
}
