//! Shared UI primitives: icons, status indicators, form groups and selectors.

pub use super::button::Button;

use gpui::{
    App, ClickEvent, Div, ElementId, Entity, Hsla, SharedString, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable,
    input::{Input, InputState},
};
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

/// 8px round status indicator.
pub fn dot(color: Hsla) -> Div {
    div()
        .w(px(8.))
        .h(px(8.))
        .rounded_full()
        .flex_shrink_0()
        .bg(color)
}

/// Small section label (12px, secondary color).
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// Segmented selector for mutually exclusive preferences.
pub fn segmented(
    id_prefix: &'static str,
    options: &[String],
    selected: usize,
    on_select: SelectHandler,
    cx: &App,
) -> Div {
    let t = cx.theme();
    // Inset track with a raised active segment; wraps at compact window sizes.
    let track = t.background;
    let mut row = div()
        .flex()
        .w_auto()
        .max_w_full()
        // Narrow panes (settings at minimum window width) must wrap instead of clipping
        // the trailing segments.
        .flex_wrap()
        .items_center()
        .gap(px(2.))
        .p(px(4.))
        .border_1()
        .border_color(t.border)
        .rounded(px(10.))
        .bg(track);
    row.style().align_self = Some(gpui::AlignSelf::FlexStart);
    for (i, label) in options.iter().enumerate() {
        let active = i == selected;
        let cb = on_select.clone();
        // Real buttons provide focus rings, Tab navigation and keyboard activation.
        let item = Button::new(ElementId::Name(format!("{id_prefix}-{i}").into()))
            .label(label.clone())
            .ghost()
            .small()
            .h(px(30.))
            .rounded(px(7.))
            .segment(active)
            .on_click(move |ev, window, app| cb(i, ev, window, app));
        row = row.child(item);
    }
    row
}

/// Single-line Input::h only affects multiline editors; use Styled::h to align
/// actual input borders with the 36px form buttons.
pub fn form_input(state: &Entity<InputState>) -> Input {
    Styled::h(Input::new(state), px(36.))
        .w_full()
        .min_w_0()
        .rounded(px(9.))
        .shadow_none()
}

/// Compact floating toolbar shared by viewer controls and badges.
pub fn overlay_chip(cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .h(px(40.))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().popover)
        .shadow(crate::theme::popup_shadow(cx.theme().is_dark()))
        .rounded(px(20.))
}

/// Raised surface shared by settings, permissions and device metadata.
pub fn form_group(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .rounded(px(14.))
        .bg(cx.theme().group_box)
        .border_1()
        .border_color(cx.theme().border)
        .shadow_xs()
}

/// Form section heading, shared by device metadata and settings.
pub fn group_title(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .pb_1()
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().foreground)
        .child(text.into())
}

/// Section identity: a tinted icon tile and a clear title/description hierarchy.
pub fn section_header(name: &'static str, title: String, description: String, cx: &App) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .w_full()
        .min_w_0()
        .min_h(px(40.))
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(40.))
                .flex_shrink_0()
                .rounded(px(12.))
                .bg(cx.theme().accent.opacity(0.12))
                .border_1()
                .border_color(cx.theme().accent.opacity(0.18))
                .child(icon(name).size(px(20.)).text_color(cx.theme().accent)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                ),
        )
}

pub fn setting_label(title: String, description: String, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
}
