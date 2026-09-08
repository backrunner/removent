//! Application button styles. Keep GPUI's keyboard/focus/tooltip behavior while
//! supplying complete normal, hover, pressed, selected, and disabled palettes.
use gpui::{
    AnyElement, App, ClickEvent, ElementId, IntoElement, RenderOnce, SharedString, StyleRefinement,
    Window, div, prelude::*, relative,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, Sizable, Size,
    button::{Button as NativeButton, ButtonCustomVariant, ButtonVariants},
};

#[derive(Clone, Copy, Default)]
enum Tone {
    #[default]
    Secondary,
    Primary,
    Ghost,
    Surface,
    Tab(bool),
    Segment(bool),
    Destructive,
}

#[derive(IntoElement)]
pub struct Button {
    inner: NativeButton,
    tone: Tone,
    label: Option<SharedString>,
    icon: Option<Icon>,
    children: Vec<AnyElement>,
    disabled: bool,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: NativeButton::new(id),
            tone: Tone::Secondary,
            label: None,
            icon: None,
            children: Vec::new(),
            disabled: false,
        }
    }
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.inner = self.inner.tooltip(text);
        self
    }
    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.inner = self.inner.on_click(f);
        self
    }
    pub fn primary(mut self) -> Self {
        self.tone = Tone::Primary;
        self
    }
    pub fn outline(mut self) -> Self {
        self.tone = Tone::Secondary;
        self
    }
    pub fn ghost(mut self) -> Self {
        self.tone = Tone::Ghost;
        self
    }
    pub fn surface(mut self) -> Self {
        self.tone = Tone::Surface;
        self
    }
    pub fn tab(mut self, selected: bool) -> Self {
        self.tone = Tone::Tab(selected);
        self
    }
    pub fn segment(mut self, selected: bool) -> Self {
        self.tone = Tone::Segment(selected);
        self
    }
    pub fn destructive(mut self) -> Self {
        self.tone = Tone::Destructive;
        self
    }
    pub fn compact(mut self) -> Self {
        self.inner = self.inner.compact();
        self
    }
}
impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}
impl Sizable for Button {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.inner = self.inner.with_size(size);
        self
    }
}
impl Disableable for Button {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}
impl ParentElement for Button {
    fn extend(&mut self, children: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(children);
    }
}
impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = cx.theme();
        let clear = t.transparent;
        let (normal, hover, pressed, foreground, border, raised) = match self.tone {
            Tone::Primary => (
                t.primary,
                t.primary_hover,
                t.primary_active,
                t.primary_foreground,
                clear,
                true,
            ),
            Tone::Secondary => (
                t.group_box,
                t.secondary_hover,
                t.secondary_active,
                t.foreground,
                t.border,
                true,
            ),
            Tone::Surface => (
                t.group_box,
                t.list_hover,
                t.secondary_active,
                t.foreground,
                t.border,
                true,
            ),
            Tone::Ghost | Tone::Tab(false) | Tone::Segment(false) => (
                clear,
                t.list_hover,
                t.secondary_active,
                t.muted_foreground,
                clear,
                false,
            ),
            Tone::Tab(true) => (
                t.group_box,
                t.secondary_hover,
                t.secondary_active,
                t.foreground,
                t.border,
                true,
            ),
            Tone::Segment(true) => (
                t.group_box,
                t.secondary_hover,
                t.secondary_active,
                t.foreground,
                t.border,
                true,
            ),
            Tone::Destructive => (
                clear,
                t.danger.opacity(0.16),
                t.danger.opacity(0.26),
                t.danger,
                clear,
                false,
            ),
        };
        let foreground = if self.disabled {
            t.muted_foreground
        } else {
            foreground
        };
        let style = ButtonCustomVariant::new(cx)
            .color(normal)
            .hover(hover)
            .active(pressed)
            .foreground(foreground)
            .border(border)
            .shadow(raised);
        self.inner
            .custom(style)
            .when(!self.disabled, |b| b.cursor_pointer())
            .when(self.disabled, |b| b.cursor_default())
            .when_some(self.icon, |b, icon| b.icon(icon.text_color(foreground)))
            // Explicit child colors also avoid gpui-component 0.5's hard-coded
            // red hover label. Only the interaction surface changes color.
            .when_some(self.label, |b, label| {
                b.child(
                    div()
                        .flex_none()
                        .line_height(relative(1.))
                        .text_color(foreground)
                        .child(label),
                )
            })
            .when(!self.children.is_empty(), |b| {
                b.child(
                    div()
                        .min_w_0()
                        .text_color(foreground)
                        .children(self.children),
                )
            })
    }
}
