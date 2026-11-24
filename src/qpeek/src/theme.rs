//! Visual tokens for QPeek's studio, limits ticket stub, and tray.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeId {
    /// Swiss Midnight Dark with balanced contrast and rounded controls.
    #[default]
    Obsidian,
    /// Swiss Editorial Light with warm archival paper and a printed ticket.
    Paper,
    /// Neo-Brutalist Bauhaus with sharp geometry and electric signals.
    Brutalist,
    /// Retro Phosphor CRT with green-black surfaces and monospace telemetry.
    Terminal,
    /// Arctic Calm with fjord tones, soft corners, and pastel signals.
    Nordic,
}

impl ThemeId {
    pub const ALL: [Self; 5] = [
        Self::Obsidian,
        Self::Paper,
        Self::Brutalist,
        Self::Terminal,
        Self::Nordic,
    ];

    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Obsidian => "obsidian",
            Self::Paper => "paper",
            Self::Brutalist => "brutalist",
            Self::Terminal => "terminal",
            Self::Nordic => "nordic",
        }
    }

    pub fn from_code(s: &str) -> Self {
        let code = s.trim();
        Self::ALL
            .into_iter()
            .find(|theme| theme.as_code().eq_ignore_ascii_case(code))
            .unwrap_or_default()
    }

    pub const fn spec(self) -> ThemeSpec {
        match self {
            Self::Obsidian => ThemeSpec {
                id: self,
                is_light: false,
                font_family: "Segoe UI",
                card_radius: 10.0,
                control_radius: 7.0,
                bar_radius: 3.0,
                bar_height: 6.0,
                border_w: 1.0,
                bg: rgb(0x0F1117),
                stub_bg: rgb(0x141824),
                notch_bg: rgb(0x080A0E),
                perf_dash: rgb(0x384158),
                card: rgb(0x181C26),
                card_border: rgb(0x282E3E),
                hover_bg: rgb(0x1F2433),
                badge_bg: rgb(0x202532),
                badge_border: rgb(0x31384B),
                track: rgb(0x252B3B),
                text_primary: rgb(0xF3F4F6),
                text_secondary: rgb(0xD1D5DB),
                text_muted: rgb(0x9CA3AF),
                text_dim: rgb(0x6B7280),
                emerald: rgb(0x10B981),
                amber: rgb(0xF59E0B),
                coral: rgb(0xEF4444),
                sky: rgb(0x38BDF8),
                active_btn_text: rgb(0x0F1117),
                monogram_bg: rgb(0x23293A),
            },
            Self::Paper => ThemeSpec {
                id: self,
                is_light: true,
                font_family: "Segoe UI",
                card_radius: 8.0,
                control_radius: 6.0,
                bar_radius: 2.5,
                bar_height: 5.0,
                border_w: 1.0,
                bg: rgb(0xF6F4EF),
                stub_bg: rgb(0xECE8DF),
                notch_bg: rgb(0xD9D3C7),
                perf_dash: rgb(0xB5ADA0),
                card: rgb(0xFFFFFF),
                card_border: rgb(0xDDD7CB),
                hover_bg: rgb(0xF0ECE3),
                badge_bg: rgb(0xEFECE5),
                badge_border: rgb(0xD2CBBF),
                track: rgb(0xE3DDD1),
                text_primary: rgb(0x18181B),
                text_secondary: rgb(0x3F3F46),
                text_muted: rgb(0x6B6860),
                text_dim: rgb(0x8C887E),
                emerald: rgb(0x059669),
                amber: rgb(0xD97706),
                coral: rgb(0xDC2626),
                sky: rgb(0x0284C7),
                active_btn_text: rgb(0xFFFFFF),
                monogram_bg: rgb(0xF2EFE9),
            },
            Self::Brutalist => ThemeSpec {
                id: self,
                is_light: false,
                font_family: "Segoe UI",
                card_radius: 2.0,
                control_radius: 1.0,
                bar_radius: 0.0,
                bar_height: 8.0,
                border_w: 2.0,
                bg: rgb(0x09090B),
                stub_bg: rgb(0x121215),
                notch_bg: rgb(0x000000),
                perf_dash: rgb(0x52525B),
                card: rgb(0x16161A),
                card_border: rgb(0x3F3F46),
                hover_bg: rgb(0x222228),
                badge_bg: rgb(0x202026),
                badge_border: rgb(0x52525B),
                track: rgb(0x27272A),
                text_primary: rgb(0xFAFAFA),
                text_secondary: rgb(0xE4E4E7),
                text_muted: rgb(0xA1A1AA),
                text_dim: rgb(0x71717A),
                emerald: rgb(0x84CC16),
                amber: rgb(0xFACC15),
                coral: rgb(0xF43F5E),
                sky: rgb(0x22D3EE),
                active_btn_text: rgb(0x09090B),
                monogram_bg: rgb(0x1E1E24),
            },
            Self::Terminal => ThemeSpec {
                id: self,
                is_light: false,
                font_family: "Consolas",
                card_radius: 4.0,
                control_radius: 3.0,
                bar_radius: 1.0,
                bar_height: 7.0,
                border_w: 1.0,
                bg: rgb(0x060B08),
                stub_bg: rgb(0x0A130D),
                notch_bg: rgb(0x020503),
                perf_dash: rgb(0x245338),
                card: rgb(0x0D1711),
                card_border: rgb(0x1D3B29),
                hover_bg: rgb(0x13241A),
                badge_bg: rgb(0x112218),
                badge_border: rgb(0x244A34),
                track: rgb(0x162C1F),
                text_primary: rgb(0xD1FAE5),
                text_secondary: rgb(0x86EFAC),
                text_muted: rgb(0x4ADE80),
                text_dim: rgb(0x2D7A4D),
                emerald: rgb(0x22C55E),
                amber: rgb(0xF59E0B),
                coral: rgb(0xFB7185),
                sky: rgb(0x2DD4BF),
                active_btn_text: rgb(0x060B08),
                monogram_bg: rgb(0x12251A),
            },
            Self::Nordic => ThemeSpec {
                id: self,
                is_light: false,
                font_family: "Segoe UI",
                card_radius: 16.0,
                control_radius: 11.0,
                bar_radius: 4.0,
                bar_height: 7.0,
                border_w: 1.0,
                bg: rgb(0x171C26),
                stub_bg: rgb(0x1C2330),
                notch_bg: rgb(0x10141C),
                perf_dash: rgb(0x3A4760),
                card: rgb(0x222A3A),
                card_border: rgb(0x313D52),
                hover_bg: rgb(0x283246),
                badge_bg: rgb(0x293448),
                badge_border: rgb(0x3B4963),
                track: rgb(0x2C374C),
                text_primary: rgb(0xECEFF4),
                text_secondary: rgb(0xD8DEE9),
                text_muted: rgb(0x9AA9C0),
                text_dim: rgb(0x6E7E98),
                emerald: rgb(0x5EEAD4),
                amber: rgb(0xFCD34D),
                coral: rgb(0xFDA4AF),
                sky: rgb(0x7DD3FC),
                active_btn_text: rgb(0x171C26),
                monogram_bg: rgb(0x2A3549),
            },
        }
    }
}

/// Theme geometry in logical pixels and palette colors as sRGB triples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeSpec {
    pub id: ThemeId,
    pub is_light: bool,
    pub font_family: &'static str,
    pub card_radius: f32,
    pub control_radius: f32,
    pub bar_radius: f32,
    pub bar_height: f32,
    pub border_w: f32,
    pub bg: (u8, u8, u8),
    pub stub_bg: (u8, u8, u8),
    pub notch_bg: (u8, u8, u8),
    pub perf_dash: (u8, u8, u8),
    pub card: (u8, u8, u8),
    pub card_border: (u8, u8, u8),
    pub hover_bg: (u8, u8, u8),
    pub badge_bg: (u8, u8, u8),
    pub badge_border: (u8, u8, u8),
    pub track: (u8, u8, u8),
    pub text_primary: (u8, u8, u8),
    pub text_secondary: (u8, u8, u8),
    pub text_muted: (u8, u8, u8),
    pub text_dim: (u8, u8, u8),
    pub emerald: (u8, u8, u8),
    pub amber: (u8, u8, u8),
    pub coral: (u8, u8, u8),
    pub sky: (u8, u8, u8),
    pub active_btn_text: (u8, u8, u8),
    pub monogram_bg: (u8, u8, u8),
}

const fn rgb(hex: u32) -> (u8, u8, u8) {
    let [_, red, green, blue] = hex.to_be_bytes();
    (red, green, blue)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::ThemeId;

    #[test]
    fn codes_round_trip_case_insensitively() {
        let codes = ["obsidian", "paper", "brutalist", "terminal", "nordic"];
        for (id, code) in ThemeId::ALL.into_iter().zip(codes) {
            assert_eq!(id.as_code(), code);
            assert_eq!(ThemeId::from_code(code), id);
            assert_eq!(ThemeId::from_code(&code.to_ascii_uppercase()), id);
        }
        assert_eq!(ThemeId::from_code("  PaPeR  "), ThemeId::Paper);
        assert_eq!(ThemeId::from_code("unknown"), ThemeId::Obsidian);
        assert_eq!(ThemeId::from_code(""), ThemeId::Obsidian);
    }

    #[test]
    fn studio_and_stub_surfaces_are_distinct_across_themes() {
        let mut surfaces = HashSet::new();
        for id in ThemeId::ALL {
            let spec = id.spec();
            assert!(surfaces.insert(spec.bg), "{id:?} studio");
            assert!(surfaces.insert(spec.stub_bg), "{id:?} stub");
        }
    }

    #[test]
    fn paper_is_the_only_light_theme() {
        for id in ThemeId::ALL {
            let spec = id.spec();
            assert_eq!(spec.id, id);
            assert_eq!(spec.is_light, id == ThemeId::Paper);
        }
    }

    #[test]
    fn terminal_uses_monospace_and_brutalist_uses_sharp_bars() {
        for id in ThemeId::ALL {
            let expected = if id == ThemeId::Terminal {
                "Consolas"
            } else {
                "Segoe UI"
            };
            assert_eq!(id.spec().font_family, expected);
        }

        let brutalist = ThemeId::Brutalist.spec();
        assert_eq!(brutalist.border_w, 2.0);
        assert_eq!(brutalist.bar_radius, 0.0);
    }
}
