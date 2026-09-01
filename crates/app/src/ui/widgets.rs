//! Shared UI widgets: icons, status dot, section label, health tri-state, segmented selector.

use gpui::{App, ClickEvent, Div, ElementId, Hsla, SharedString, Window, div, prelude::*, px};
use gpui_component::{ActiveTheme, Icon, Sizable};
use std::rc::Rc;

/// Segmented selector callback.
pub type SelectHandler = Rc<dyn Fn(usize, &ClickEvent, &mut Window, &mut App)>;

/// Load an embedded Lucide-style icon (crates/app/assets/icons).
pub fn icon(name: &'static str) -> Icon {
    Icon::empty().path(format!("icons/{name}.svg"))
}

pub fn icon_16(name: &'static str) -> Icon {
    icon(name).small()
}

/// Connection health tri-state (ui-design §2.1: green/yellow/red).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Good,
    Fair,
    Poor,
    Unknown,
}

impl Health {
    pub fn color(self, cx: &App) -> Hsla {
        let t = &cx.theme().colors;
        match self {
            Self::Good => t.success,
            Self::Fair => t.warning,
            Self::Poor => t.danger,
            Self::Unknown => t.muted_foreground,
        }
    }
}

/// 8px round status indicator.
pub fn dot(color: Hsla) -> Div {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_shrink_0()
        .bg(color)
}

/// Small section label (11px, secondary color).
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(11.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Outlined segmented selector (used for admission mode / theme / language switching).
pub fn segmented(
    id_prefix: &'static str,
    options: &[String],
    selected: usize,
    on_select: SelectHandler,
    cx: &App,
) -> Div {
    let t = cx.theme();
    // The track must read as a separate layer from grouped_card (secondary background):
    // dark mode uses the deeper window background as track; light mode uses a gray track
    // with a white selected pill.
    let (track, pill) = if t.is_dark() {
        (t.background, t.secondary_active)
    } else {
        (t.list_active, t.popover)
    };
    let mut row = div()
        .flex()
        // Narrow panes (settings at minimum window width) must wrap instead of clipping
        // the trailing segments.
        .flex_wrap()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(10.))
        .bg(track);
    for (i, label) in options.iter().enumerate() {
        let active = i == selected;
        let cb = on_select.clone();
        let mut item = div()
            .id(ElementId::Name(format!("{id_prefix}-{i}").into()))
            .px_3()
            .py_1()
            .rounded(px(8.))
            .text_size(px(12.))
            .cursor_pointer()
            .child(label.clone())
            .on_click(move |ev, window, app| cb(i, ev, window, app));
        if active {
            item = item.bg(pill).text_color(t.foreground).shadow_sm();
        } else {
            item = item
                .text_color(t.muted_foreground)
                .hover(|s| s.text_color(t.foreground));
        }
        row = row.child(item);
    }
    row
}

/// Floating frosted-glass toolbar container (viewer top bar / badge base), fully rounded pill.
pub fn overlay_chip() -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(28.))
        .rounded(px(14.))
}

/// macOS inset-grouped card container: surface background, 12px radius, low-contrast border.
/// Reused by the detail page metadata and the settings groups.
pub fn grouped_card(cx: &App) -> Div {
    let t = cx.theme();
    div()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .bg(t.secondary)
        .border_1()
        .border_color(t.border)
}

/// 11px secondary-color caption above a grouped card (macOS form group label).
pub fn card_title(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_1()
        .pb_1()
        .text_size(px(11.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
