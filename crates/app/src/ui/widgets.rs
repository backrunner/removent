//! Shared UI primitives: icons, status indicators, form groups and selectors.

use gpui::{
    App, ClickEvent, Div, ElementId, Entity, Hsla, SharedString, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Icon, Sizable,
    button::{Button, ButtonVariants},
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
    // A subtle shared track keeps the options visually related on a plain form surface.
    let (track, pill) = if t.is_dark() {
        (t.secondary, t.secondary_active)
    } else {
        (t.list_active, t.popover)
    };
    let mut row = div()
        .flex()
        .w_auto()
        .max_w_full()
        // Narrow panes (settings at minimum window width) must wrap instead of clipping
        // the trailing segments.
        .flex_wrap()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(6.))
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
            .h(px(24.))
            .rounded(px(4.))
            .when(active, |el| el.bg(pill).text_color(t.foreground))
            .when(!active, |el| el.text_color(t.muted_foreground))
            .on_click(move |ev, window, app| cb(i, ev, window, app));
        row = row.child(item);
    }
    row
}

/// Single-line Input::h only affects multiline editors; use Styled::h to align
/// actual input borders with the 32px form buttons.
pub fn form_input(state: &Entity<InputState>) -> Input {
    Styled::h(Input::new(state), px(32.))
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

/// Plain form group with a hairline separating the heading from its rows.
pub fn form_group(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(cx.theme().border)
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
