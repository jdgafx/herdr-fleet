//! Colour themes: catppuccin Mocha (dark), Latte (light), and a 16-colour fallback
//! that uses only the terminal's own palette.

use clap::ValueEnum;
use ratatui::style::Color;

/// `--theme` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThemeName {
    /// catppuccin Mocha (dark)
    Mocha,
    /// catppuccin Latte (light)
    Latte,
    /// the terminal's own 16 colours, for terminals without truecolour
    Ansi16,
}

/// The colours the dashboard draws with. Each role has one meaning everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Title badge, focus marker, spinner accent.
    pub accent: Color,
    /// Body text.
    pub text: Color,
    /// Hints, labels and idle chrome.
    pub muted: Color,
    /// Box borders.
    pub border: Color,
    /// Text drawn on a coloured pill.
    pub on_accent: Color,
    pub working: Color,
    pub idle: Color,
    pub done: Color,
    pub blocked: Color,
    /// Context between 50 and 79 %.
    pub warn: Color,
    /// Stalled, looping, dialog, drift, and context at 80 % or more.
    pub alarm: Color,
    /// The empty part of a gauge.
    pub surface: Color,
}

impl Theme {
    pub fn named(name: ThemeName) -> Theme {
        match name {
            ThemeName::Mocha => from_flavor(&catppuccin::PALETTE.mocha),
            ThemeName::Latte => from_flavor(&catppuccin::PALETTE.latte),
            ThemeName::Ansi16 => Theme {
                accent: Color::Cyan,
                text: Color::White,
                muted: Color::DarkGray,
                border: Color::Gray,
                on_accent: Color::Black,
                working: Color::Yellow,
                idle: Color::Cyan,
                done: Color::Green,
                blocked: Color::Red,
                warn: Color::Yellow,
                alarm: Color::Red,
                surface: Color::DarkGray,
            },
        }
    }
}

fn from_flavor(flavor: &catppuccin::Flavor) -> Theme {
    let c = &flavor.colors;
    let rgb = |color: catppuccin::Color| Color::from(color);
    Theme {
        accent: rgb(c.mauve),
        text: rgb(c.text),
        muted: rgb(c.overlay1),
        border: rgb(c.surface2),
        on_accent: rgb(c.base),
        working: rgb(c.yellow),
        idle: rgb(c.blue),
        done: rgb(c.green),
        blocked: rgb(c.red),
        warn: rgb(c.peach),
        alarm: rgb(c.red),
        surface: rgb(c.surface1),
    }
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn mocha_and_latte_are_truecolour_and_ansi16_is_not() {
        let mocha = Theme::named(ThemeName::Mocha);
        let latte = Theme::named(ThemeName::Latte);
        let ansi = Theme::named(ThemeName::Ansi16);
        assert!(matches!(mocha.accent, Color::Rgb(..)));
        assert!(matches!(latte.accent, Color::Rgb(..)));
        assert_ne!(mocha.text, latte.text);
        for c in [ansi.accent, ansi.working, ansi.blocked, ansi.muted] {
            assert!(
                !matches!(c, Color::Rgb(..)),
                "{c:?} is not a 16-colour value"
            );
        }
    }
}
