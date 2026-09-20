use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Light,
    Ice,
    GrokNight,
    Frost,
    Ember,
    Mono,
    Solarized,
}

impl ThemeName {
    /// ICE (glacier) is the default theme.
    pub const DEFAULT: ThemeName = ThemeName::Ice;

    pub fn all() -> &'static [ThemeName] {
        &[
            ThemeName::Light,
            ThemeName::Ice,
            ThemeName::GrokNight,
            ThemeName::Frost,
            ThemeName::Ember,
            ThemeName::Mono,
            ThemeName::Solarized,
        ]
    }

    pub fn id(self) -> &'static str {
        match self {
            ThemeName::Light => "light",
            ThemeName::Ice => "ice",
            ThemeName::GrokNight => "groknight",
            ThemeName::Frost => "frost",
            ThemeName::Ember => "ember",
            ThemeName::Mono => "mono",
            ThemeName::Solarized => "solarized",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" | "white" | "day" | "default" => Some(Self::Light),
            "ice" => Some(Self::Ice),
            "grok" | "groknight" | "night" => Some(Self::GrokNight),
            "frost" | "blue" => Some(Self::Frost),
            "ember" | "warm" => Some(Self::Ember),
            "mono" | "bw" => Some(Self::Mono),
            "solarized" | "solar" => Some(Self::Solarized),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        let all = Self::all();
        let i = all.iter().position(|t| *t == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }

    pub fn palette(self) -> Theme {
        match self {
            Self::Light => Theme::light(),
            Self::Ice => Theme::ice(),
            Self::GrokNight => Theme::grok_night(),
            Self::Frost => Theme::frost(),
            Self::Ember => Theme::ember(),
            Self::Mono => Theme::mono(),
            Self::Solarized => Theme::solarized(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub border: Color,
    pub border_focus: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_dim: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub ice: Color,
    pub user: Color,
}

impl Theme {
    /// Default light/white theme: paper background, ink text, glacier accent.
    pub fn light() -> Self {
        Self {
            bg: Color::Rgb(250, 251, 252),
            surface: Color::Rgb(240, 243, 246),
            border: Color::Rgb(210, 218, 224),
            border_focus: Color::Rgb(20, 140, 190),
            text: Color::Rgb(24, 34, 44),
            muted: Color::Rgb(110, 124, 138),
            accent: Color::Rgb(10, 132, 178),
            accent_dim: Color::Rgb(150, 195, 215),
            ok: Color::Rgb(20, 150, 100),
            warn: Color::Rgb(180, 120, 20),
            err: Color::Rgb(200, 50, 70),
            ice: Color::Rgb(30, 150, 200),
            user: Color::Rgb(40, 90, 150),
        }
    }

    /// Solarized-light inspired palette.
    pub fn solarized() -> Self {
        Self {
            bg: Color::Rgb(253, 246, 227),
            surface: Color::Rgb(238, 232, 213),
            border: Color::Rgb(203, 197, 178),
            border_focus: Color::Rgb(38, 139, 210),
            text: Color::Rgb(88, 110, 117),
            muted: Color::Rgb(147, 161, 161),
            accent: Color::Rgb(38, 139, 210),
            accent_dim: Color::Rgb(42, 161, 152),
            ok: Color::Rgb(133, 153, 0),
            warn: Color::Rgb(181, 137, 0),
            err: Color::Rgb(220, 50, 47),
            ice: Color::Rgb(42, 161, 152),
            user: Color::Rgb(108, 113, 196),
        }
    }

    /// ICE theme: deep navy + glacier cyan.
    pub fn ice() -> Self {
        Self {
            bg: Color::Rgb(6, 16, 24),
            surface: Color::Rgb(10, 28, 40),
            border: Color::Rgb(28, 72, 92),
            border_focus: Color::Rgb(120, 220, 255),
            text: Color::Rgb(226, 244, 252),
            muted: Color::Rgb(110, 156, 176),
            accent: Color::Rgb(80, 210, 255),
            accent_dim: Color::Rgb(32, 120, 160),
            ok: Color::Rgb(64, 220, 170),
            warn: Color::Rgb(240, 196, 80),
            err: Color::Rgb(255, 96, 120),
            ice: Color::Rgb(160, 236, 255),
            user: Color::Rgb(140, 200, 255),
        }
    }

    pub fn grok_night() -> Self {
        Self {
            bg: Color::Rgb(12, 12, 16),
            surface: Color::Rgb(22, 22, 28),
            border: Color::Rgb(48, 44, 58),
            border_focus: Color::Rgb(196, 92, 196),
            text: Color::Rgb(230, 228, 236),
            muted: Color::Rgb(140, 136, 152),
            accent: Color::Rgb(212, 88, 212),
            accent_dim: Color::Rgb(148, 64, 156),
            ok: Color::Rgb(80, 200, 140),
            warn: Color::Rgb(232, 176, 72),
            err: Color::Rgb(232, 88, 104),
            ice: Color::Rgb(140, 220, 255),
            user: Color::Rgb(120, 180, 255),
        }
    }

    pub fn frost() -> Self {
        Self {
            bg: Color::Rgb(8, 16, 24),
            surface: Color::Rgb(14, 28, 40),
            border: Color::Rgb(32, 64, 88),
            border_focus: Color::Rgb(120, 210, 255),
            text: Color::Rgb(220, 236, 246),
            muted: Color::Rgb(110, 150, 170),
            accent: Color::Rgb(90, 200, 255),
            accent_dim: Color::Rgb(40, 120, 170),
            ok: Color::Rgb(80, 210, 160),
            warn: Color::Rgb(240, 190, 80),
            err: Color::Rgb(240, 90, 110),
            ice: Color::Rgb(180, 240, 255),
            user: Color::Rgb(160, 200, 255),
        }
    }

    pub fn ember() -> Self {
        Self {
            bg: Color::Rgb(18, 10, 8),
            surface: Color::Rgb(32, 18, 14),
            border: Color::Rgb(80, 40, 28),
            border_focus: Color::Rgb(255, 140, 80),
            text: Color::Rgb(252, 236, 224),
            muted: Color::Rgb(168, 120, 96),
            accent: Color::Rgb(255, 120, 64),
            accent_dim: Color::Rgb(160, 64, 32),
            ok: Color::Rgb(120, 200, 120),
            warn: Color::Rgb(255, 196, 64),
            err: Color::Rgb(255, 80, 80),
            ice: Color::Rgb(255, 200, 140),
            user: Color::Rgb(255, 180, 120),
        }
    }

    pub fn mono() -> Self {
        Self {
            bg: Color::Rgb(10, 10, 10),
            surface: Color::Rgb(18, 18, 18),
            border: Color::Rgb(60, 60, 60),
            border_focus: Color::Rgb(230, 230, 230),
            text: Color::Rgb(230, 230, 230),
            muted: Color::Rgb(130, 130, 130),
            accent: Color::Rgb(240, 240, 240),
            accent_dim: Color::Rgb(90, 90, 90),
            ok: Color::Rgb(200, 200, 200),
            warn: Color::Rgb(180, 180, 180),
            err: Color::Rgb(160, 160, 160),
            ice: Color::Rgb(210, 210, 210),
            user: Color::Rgb(220, 220, 220),
        }
    }

    pub fn title(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    pub fn dim(&self) -> Style {
        Style::default().fg(self.muted)
    }

    pub fn body(&self) -> Style {
        Style::default().fg(self.text)
    }
}

pub fn context_window_for(model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    if m.contains("qwen3.8") || m.contains("qwen3-8") {
        131_072
    } else if m.contains("gpt-4o") || m.contains("grok") {
        128_000
    } else if m.contains("claude") {
        200_000
    } else if m.contains("gemini") {
        1_000_000
    } else {
        128_000
    }
}

pub fn estimate_tokens(s: &str) -> u32 {
    // crude 4-chars ≈ 1 token
    (s.len() as u32 / 4).max(1)
}
