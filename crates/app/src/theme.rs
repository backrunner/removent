//! Neutral macOS surfaces, restrained separators, and clear interaction states.

use gpui::{App, BoxShadow, Hsla, point, px, rgba};

/// Opaque token.
fn c(hex: u32) -> Hsla {
    gpui::rgb(hex).into()
}

/// Token with an alpha channel (RGBA u32).
fn ca(hex: u32) -> Hsla {
    rgba(hex).into()
}

/// Semantic colors from design doc §2.1 (dark / light variants).
pub struct Palette {
    pub background: Hsla,
    pub surface: Hsla,
    pub overlay: Hsla,
    pub border: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            background: c(0x1E2025),
            surface: c(0x292C32),
            overlay: ca(0x2D3037F5),
            border: ca(0xFFFFFF16),
            text_primary: c(0xF5F5F7),
            text_secondary: c(0xA8ABB3),
            accent: c(0x82B5FF),
            success: c(0x30D158),
            warning: c(0xFF9F0A),
            danger: c(0xFF453A),
        }
    } else {
        Palette {
            background: c(0xF5F5F7),
            surface: c(0xF5F5F7),
            overlay: ca(0xFFFFFFF2),
            border: ca(0x1C213014),
            text_primary: c(0x202127),
            text_secondary: c(0x686B74),
            accent: c(0x0865DB),
            // In light mode status colors also serve as small text (status bar/tags); the stock
            // system colors lack contrast (2.2:1), so use darkened shades to reach ≥4.5:1 (WCAG AA).
            success: c(0x1F7A38),
            warning: c(0xC93400),
            danger: c(0xD70015),
        }
    }
}

/// Full-screen modal scrim: distinct from the overlay token used under floating
/// chips/popups, so light mode's 95% white does not wash out the background content.
pub fn scrim(dark: bool) -> Hsla {
    if dark { ca(0x00000099) } else { ca(0x00000040) }
}

/// Floating sheets need separation from content without a bright rim or glow.
pub fn popup_shadow(dark: bool) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: if dark { ca(0x00000052) } else { ca(0x17203326) },
        offset: point(px(0.), px(8.)),
        blur_radius: px(24.),
        spread_radius: px(-3.),
    }]
}

/// Write the tokens into the gpui-component global Theme. Dark is the default (§1 principle 6).
pub fn apply(dark: bool, cx: &mut App) {
    let p = palette(dark);
    let theme = gpui_component::theme::Theme::global_mut(cx);
    let t = &mut theme.colors;

    // Keep the window backing opaque: GPUI's blurred mode on newer macOS
    // can leave sharp background windows visible through controls.
    t.background = p.background;
    t.foreground = p.text_primary;
    t.border = p.border;
    t.muted = p.surface;
    t.muted_foreground = p.text_secondary;
    t.secondary = p.surface;
    t.secondary_hover = if dark { c(0x41454E) } else { c(0xE8ECF2) };
    t.secondary_active = if dark { c(0x4B505A) } else { c(0xDEE4EC) };
    t.secondary_foreground = p.text_primary;
    t.accent = p.accent;
    t.accent_foreground = c(0xFFFFFF);
    // Filled buttons need stronger contrast for their small white labels than
    // the accent used by focus rings and switches.
    t.primary = if dark { c(0x306ACE) } else { p.accent };
    t.primary_hover = if dark { c(0x3871D6) } else { c(0x2D6DE4) };
    t.primary_active = if dark { c(0x2354A6) } else { c(0x194BA9) };
    t.primary_foreground = c(0xFFFFFF);
    t.danger = p.danger;
    t.danger_hover = p.danger;
    t.danger_active = p.danger;
    t.danger_foreground = c(0xFFFFFF);
    t.success = p.success;
    t.success_foreground = c(0xFFFFFF);
    t.success_hover = p.success;
    t.success_active = p.success;
    t.warning = p.warning;
    t.warning_foreground = c(0xFFFFFF);
    t.warning_hover = p.warning;
    t.warning_active = p.warning;
    t.info = p.accent;
    t.info_foreground = c(0xFFFFFF);
    t.info_hover = p.accent;
    t.info_active = p.accent;
    t.overlay = p.overlay;
    t.popover = if dark { c(0x2B2E35) } else { c(0xFFFFFF) };
    t.popover_foreground = p.text_primary;
    t.input = p.border;
    t.caret = p.text_primary;
    t.ring = p.accent;
    t.selection = if dark { ca(0x0A84FF44) } else { ca(0x007AFF33) };
    t.list = p.background;
    t.list_hover = if dark { c(0x3A3E47) } else { c(0xE8ECF2) };
    t.list_active = if dark { c(0x4B505A) } else { c(0xDEE4EC) };
    t.list_active_border = p.border;
    t.list_even = p.background;
    t.list_head = p.surface;
    t.sidebar = if dark { c(0x23262D) } else { c(0xECEEF1) };
    t.sidebar_foreground = p.text_primary;
    t.sidebar_border = p.border;
    t.sidebar_accent = if dark { c(0x41454E) } else { c(0xE8ECF2) };
    t.sidebar_accent_foreground = p.text_primary;
    t.sidebar_primary = p.accent;
    t.sidebar_primary_foreground = c(0xFFFFFF);
    t.title_bar = t.sidebar;
    t.title_bar_border = p.border;
    t.tab = p.surface;
    t.tab_active = p.background;
    t.tab_active_foreground = p.text_primary;
    t.tab_foreground = p.text_secondary;
    t.tab_bar = p.surface;
    t.tab_bar_segmented = p.surface;
    t.group_box = if dark { c(0x2B2E35) } else { c(0xFFFFFF) };
    t.group_box_foreground = p.text_primary;
    t.skeleton = if dark { c(0x4B505A) } else { c(0xDEE4EC) };
    t.switch = if dark { c(0x4A4D55) } else { c(0xD1D1D6) };
    t.switch_thumb = c(0xFFFFFF);
    t.slider_bar = if dark { c(0x4A4D55) } else { c(0xD1D1D6) };
    t.slider_thumb = p.accent;
    t.progress_bar = p.accent;
    t.scrollbar = p.background;
    t.scrollbar_thumb = if dark { ca(0x5A5A5ECC) } else { ca(0xC1C1C6CC) };
    t.scrollbar_thumb_hover = if dark { c(0x6A6A70) } else { c(0xAEAEB4) };
    t.window_border = p.border;
    t.drag_border = p.accent;
    t.drop_target = if dark { ca(0x0A84FF22) } else { ca(0x007AFF18) };
    t.link = p.accent;
    t.link_hover = p.accent;
    t.link_active = p.accent;
    t.description_list_label = p.surface;
    t.description_list_label_foreground = p.text_secondary;
    t.table = p.background;
    t.table_head = p.surface;
    t.table_head_foreground = p.text_secondary;
    t.table_hover = t.list_hover;
    t.table_active = t.list_active;
    t.table_active_border = p.border;
    t.table_even = p.background;
    t.table_row_border = p.border;
    t.accordion = p.surface;
    t.accordion_hover = t.list_hover;
    t.tiles = p.surface;
    t.bullish = p.success;
    t.bearish = p.danger;

    theme.shadow = true;
    theme.radius = px(9.);
    theme.radius_lg = px(16.);
    theme.font_size = px(13.);
}
