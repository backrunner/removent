//! Shared UI primitives: icons, status indicators, form groups and selectors.

pub use super::button::Button;

use gpui::{
    App, ClickEvent, Div, ElementId, Entity, Hsla, SharedString, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable,
    input::{Input, InputState},
};
use std::rc::Rc;

/// Segmented selector callback.
pub type SelectHandler = Rc<dyn Fn(usize, &ClickEvent, &mut Window, &mut App)>;

/// Load an embedded Lucide-style icon (apps/desktop/assets/icons).
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
    segmented_with_disabled(id_prefix, options, selected, false, on_select, cx)
}

pub fn segmented_with_disabled(
    id_prefix: &'static str,
    options: &[String],
    selected: usize,
    disabled: bool,
    on_select: SelectHandler,
    cx: &App,
) -> Div {
    let t = cx.theme();
    let text_system = cx.text_system();
    let font_id = text_system.resolve_font(&gpui::font(t.font_family.clone()));
    let segment_width = options
        .iter()
        .map(|label| {
            label
                .chars()
                .map(|ch| {
                    text_system
                        .advance(font_id, px(12.), ch)
                        .map(|advance| advance.width)
                        .unwrap_or(px(12.))
                })
                .fold(px(0.), |width, advance| width + advance)
        })
        .fold(px(0.), |width, next| width.max(next))
        + px(24.);
    // Related choices remain grouped, with equal widths measured from their labels.
    let track = t.background;
    let mut row = div()
        .flex()
        .w_auto()
        .max_w_full()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .border_1()
        .border_color(t.border)
        .rounded(px(7.))
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
            .w(segment_width)
            .h(px(28.))
            .rounded(px(6.))
            .segment(active)
            .disabled(disabled)
            .on_click(move |ev, window, app| cb(i, ev, window, app));
        #[cfg(test)]
        let item = div()
            .debug_selector(|| format!("{id_prefix}-{i}"))
            .child(item);
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
        .rounded(px(8.))
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

/// Content grouping stays quiet; depth is reserved for navigation and overlays.
pub fn form_group(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .rounded(px(12.))
        .bg(cx.theme().group_box)
        .border_1()
        .border_color(cx.theme().border)
        .shadow_sm()
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

/// Section identity uses a monochrome symbol and a clear title/description hierarchy.
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
                .size(px(28.))
                .flex_shrink_0()
                .child(
                    icon(name)
                        .size(px(22.))
                        .text_color(cx.theme().muted_foreground),
                ),
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
