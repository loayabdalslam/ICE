//! Colour themes. The REPL draws on the terminal's own background (like
//! Claude Code), so each theme is tuned for a dark or a light terminal and
//! only sets foregrounds plus diff backgrounds.

use crossterm::style::Color;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemeName {
    Ice,
    Light,
    Frost,
    Ember,
    GrokNight,
    Mono,
    Solarized,
    DarkDaltonized,
    LightDaltonized,
}

impl ThemeName {
    /// ICE (glacier, dark terminal) is the default theme.
    pub const DEFAULT: ThemeName = ThemeName::Ice;

    pub fn all() -> &'static [ThemeName] {
        &[
            ThemeName::Ice,
            ThemeName::Light,
            ThemeName::Frost,
            ThemeName::Ember,
            ThemeName::GrokNight,
            ThemeName::Mono,
            ThemeName::Solarized,
            ThemeName::DarkDaltonized,
            ThemeName::LightDaltonized,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            ThemeName::Ice => "ice",
            ThemeName::Light => "light",
            ThemeName::Frost => "frost",
            ThemeName::Ember => "ember",
            ThemeName::GrokNight => "groknight",
            ThemeName::Mono => "mono",
            ThemeName::Solarized => "solarized",
            ThemeName::DarkDaltonized => "dark-daltonized",
            ThemeName::LightDaltonized => "light-daltonized",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ThemeName::Ice => "Dark mode · glacier cyan (default)",
            ThemeName::Light => "Light mode",
            ThemeName::Frost => "Dark mode · cool blue",
            ThemeName::Ember => "Dark mode · warm orange",
            ThemeName::GrokNight => "Dark mode · violet",
            ThemeName::Mono => "Monochrome",
            ThemeName::Solarized => "Light mode · solarized",
            ThemeName::DarkDaltonized => "Dark mode (colorblind-friendly)",
            ThemeName::LightDaltonized => "Light mode (colorblind-friendly)",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ice" | "dark" | "default" => Some(Self::Ice),
            "light" | "white" | "day" => Some(Self::Light),
            "frost" | "blue" => Some(Self::Frost),
            "ember" | "warm" => Some(Self::Ember),
            "grok" | "groknight" | "night" => Some(Self::GrokNight),
            "mono" | "bw" => Some(Self::Mono),
            "solarized" | "solar" => Some(Self::Solarized),
            "dark-daltonized" => Some(Self::DarkDaltonized),
            "light-daltonized" => Some(Self::LightDaltonized),
            _ => None,
        }
    }

    pub fn palette(self) -> Theme {
        let dark = Theme {
            accent: rgb(80, 210, 255),
            dim: rgb(136, 146, 156),
            ok: rgb(78, 186, 101),
            err: rgb(255, 107, 128),
            warn: rgb(255, 193, 7),
            code: rgb(177, 185, 249),
            code_block: rgb(200, 210, 220),
            add_bg: rgb(34, 92, 43),
            del_bg: rgb(122, 41, 54),
            border: rgb(96, 110, 124),
            user: rgb(160, 168, 176),
            bash: rgb(253, 93, 177),
            memory: rgb(175, 135, 255),
            plan: rgb(72, 150, 140),
            edits: rgb(175, 135, 255),
            floe_top: rgb(160, 236, 255),
            floe_face: rgb(80, 210, 255),
            floe_side: rgb(32, 120, 160),
            bg: rgb(6, 16, 24),
            surface: rgb(10, 28, 40),
            gauge: rgb(32, 120, 160),
            highlight: rgb(226, 244, 252),
        };
        let light = Theme {
            accent: rgb(10, 120, 170),
            dim: rgb(102, 102, 102),
            ok: rgb(44, 122, 57),
            err: rgb(171, 43, 63),
            warn: rgb(150, 108, 30),
            code: rgb(87, 105, 247),
            code_block: rgb(60, 60, 60),
            add_bg: rgb(105, 219, 124),
            del_bg: rgb(255, 168, 180),
            border: rgb(153, 153, 153),
            user: rgb(90, 90, 90),
            bash: rgb(255, 0, 135),
            memory: rgb(130, 80, 223),
            plan: rgb(0, 102, 102),
            edits: rgb(130, 80, 223),
            floe_top: rgb(30, 150, 200),
            floe_face: rgb(10, 120, 170),
            floe_side: rgb(8, 80, 120),
            bg: rgb(250, 251, 252),
            surface: rgb(232, 238, 243),
            gauge: rgb(150, 195, 215),
            highlight: rgb(255, 255, 255),
        };
        match self {
            Self::Ice => dark,
            Self::Light => light,
            Self::Frost => Theme {
                accent: rgb(90, 200, 255),
                code: rgb(160, 200, 255),
                floe_top: rgb(180, 240, 255),
                ..dark
            },
            Self::Ember => Theme {
                accent: rgb(255, 140, 80),
                code: rgb(255, 200, 140),
                floe_top: rgb(255, 200, 140),
                floe_face: rgb(255, 120, 64),
                floe_side: rgb(160, 64, 32),
                ..dark
            },
            Self::GrokNight => Theme {
                accent: rgb(212, 88, 212),
                code: rgb(200, 160, 255),
                floe_top: rgb(230, 170, 240),
                floe_face: rgb(212, 88, 212),
                floe_side: rgb(120, 50, 130),
                ..dark
            },
            Self::Mono => Theme {
                accent: Color::Reset,
                dim: rgb(128, 128, 128),
                ok: Color::Reset,
                err: Color::Reset,
                warn: Color::Reset,
                code: Color::Reset,
                code_block: Color::Reset,
                add_bg: rgb(60, 60, 60),
                del_bg: rgb(30, 30, 30),
                border: rgb(128, 128, 128),
                user: rgb(128, 128, 128),
                bash: Color::Reset,
                memory: Color::Reset,
                plan: Color::Reset,
                edits: Color::Reset,
                floe_top: rgb(220, 220, 220),
                floe_face: rgb(160, 160, 160),
                floe_side: rgb(90, 90, 90),
                bg: rgb(10, 10, 10),
                surface: rgb(24, 24, 24),
                gauge: rgb(90, 90, 90),
                highlight: rgb(240, 240, 240),
            },
            Self::Solarized => Theme {
                accent: rgb(38, 139, 210),
                code: rgb(108, 113, 196),
                ok: rgb(133, 153, 0),
                err: rgb(220, 50, 47),
                warn: rgb(181, 137, 0),
                ..light
            },
            Self::DarkDaltonized => Theme {
                ok: rgb(51, 153, 255),
                err: rgb(255, 102, 102),
                add_bg: rgb(0, 68, 102),
                del_bg: rgb(102, 0, 0),
                ..dark
            },
            Self::LightDaltonized => Theme {
                ok: rgb(0, 102, 153),
                err: rgb(204, 0, 0),
                add_bg: rgb(153, 204, 255),
                del_bg: rgb(255, 204, 204),
                ..light
            },
        }
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub accent: Color,
    pub dim: Color,
    pub ok: Color,
    pub err: Color,
    pub warn: Color,
    pub code: Color,
    pub code_block: Color,
    pub add_bg: Color,
    pub del_bg: Color,
    pub border: Color,
    pub user: Color,
    pub bash: Color,
    pub memory: Color,
    pub plan: Color,
    pub edits: Color,
    pub floe_top: Color,
    pub floe_face: Color,
    pub floe_side: Color,
    /// Page background (FLOE's eyes, gauge text) and panel surfaces.
    pub bg: Color,
    pub surface: Color,
    pub gauge: Color,
    pub highlight: Color,
}

/// True-colour support; otherwise RGB is mapped to the 256-colour cube so
/// Apple Terminal and older terminals don't print garbage.
pub fn truecolor() -> bool {
    matches!(
        std::env::var("COLORTERM").as_deref(),
        Ok("truecolor") | Ok("24bit")
    ) || std::env::var("TERM_PROGRAM")
        .map(|t| t != "Apple_Terminal")
        .unwrap_or(false)
        || std::env::var_os("WT_SESSION").is_some()
        || cfg!(windows)
}

pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

pub fn to_ansi256(c: Color) -> Color {
    match c {
        Color::Rgb { r, g, b } => {
            let q = |v: u8| -> u8 {
                if v < 48 {
                    0
                } else if v < 115 {
                    1
                } else {
                    (v - 35) / 40
                }
            };
            let (qr, qg, qb) = (q(r), q(g), q(b));
            // Greys map better onto the grey ramp.
            if r.abs_diff(g) < 12 && g.abs_diff(b) < 12 {
                let avg = (r as u16 + g as u16 + b as u16) / 3;
                if avg < 8 {
                    return Color::AnsiValue(16);
                }
                if avg > 238 {
                    return Color::AnsiValue(231);
                }
                return Color::AnsiValue(232 + ((avg - 8) / 10).min(23) as u8);
            }
            Color::AnsiValue(16 + 36 * qr + 6 * qg + qb)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_parse_and_map_to_256() {
        for t in ThemeName::all() {
            assert_eq!(ThemeName::parse(t.id()), Some(*t));
        }
        assert_eq!(to_ansi256(rgb(255, 0, 0)), Color::AnsiValue(196));
        assert!(matches!(
            to_ansi256(rgb(128, 128, 128)),
            Color::AnsiValue(232..=255)
        ));
    }
}
