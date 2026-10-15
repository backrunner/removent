//! Shared inline TUI components. The terminal owns its colors and scrollback;
//! we own only a bounded prompt region, never an alternate screen.
mod form;
pub(crate) mod progress;
mod region;
pub(crate) mod results;
mod select;

use crate::terminal::{clean, wrap};
pub(crate) use form::{confirm, input, input_validated};
pub(crate) use region::Session;
pub(crate) use select::{Item, choose};
use std::io::{IsTerminal, Write};

#[derive(Clone, Copy)]
pub(crate) enum Tone {
    Plain,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

pub(crate) fn enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var("TERM").as_deref() != Ok("dumb")
}

pub(crate) fn paint(value: impl AsRef<str>, tone: Tone) -> String {
    let value = value.as_ref();
    if !enabled()
        || std::env::var_os("NO_COLOR").is_some()
        || std::env::var("CLICOLOR").as_deref() == Ok("0")
    {
        return value.to_owned();
    }
    // ANSI semantic colors inherit the user's terminal palette, on both light
    // and dark backgrounds. Never force an RGB background behind text.
    let code = match tone {
        Tone::Plain => "0",
        Tone::Muted => "90",
        Tone::Accent => "36",
        Tone::Success => "32",
        Tone::Warning => "33",
        Tone::Danger => "31",
    };
    format!("\x1b[{code}m{value}\x1b[0m")
}

pub(crate) fn dimensions() -> (usize, usize) {
    let (rows, columns) = console::Term::stderr().size();
    (
        usize::from(columns).saturating_sub(3).clamp(12, 76),
        usize::from(rows).max(6),
    )
}

pub(crate) fn fit(value: &str, width: usize) -> String {
    let value = clean(value);
    if console::measure_text_width(&value) <= width {
        return value;
    }
    if width == 0 {
        return String::new();
    }
    let mut line = wrap(&value, width.saturating_sub(1).max(1)).remove(0);
    while console::measure_text_width(&line) >= width
        || line.ends_with(['（', '【', '《', '「', '『', '“', '‘', '(', '[', '{'])
    {
        line.pop();
    }
    format!("{line}…")
}

pub(crate) fn badge(value: &str, tone: Tone) -> String {
    paint(format!("[ {} ]", clean(value)), tone)
}

pub(crate) struct Card {
    pub lines: Vec<String>,
    pub inner: usize,
}

impl Card {
    pub fn new(title: &str, width: usize, tone: Tone) -> Self {
        let inner = width.saturating_sub(4).max(4);
        let title = fit(title, inner.saturating_sub(2));
        let remaining = inner.saturating_sub(console::measure_text_width(&title) + 1);
        let mut card = Self {
            lines: vec![format!(
                "  {} {} {}",
                paint("╭─", tone),
                paint(title, tone),
                paint("─".repeat(remaining) + "╮", tone)
            )],
            inner,
        };
        card.line("");
        card
    }

    pub fn line(&mut self, text: impl AsRef<str>) {
        let text = text.as_ref();
        let padding = self.inner.saturating_sub(console::measure_text_width(text));
        self.lines.push(format!(
            "  {} {text}{} {}",
            paint("│", Tone::Muted),
            " ".repeat(padding),
            paint("│", Tone::Muted)
        ));
    }

    pub fn text(&mut self, text: &str, tone: Tone) {
        for line in wrap(text, self.inner) {
            self.line(paint(line, tone));
        }
    }

    pub fn divider(&mut self) {
        self.lines.push(format!(
            "  {}{}{}",
            paint("├", Tone::Muted),
            paint("─".repeat(self.inner + 2), Tone::Muted),
            paint("┤", Tone::Muted)
        ));
    }

    pub fn field(&mut self, label: &str, value: &str, tone: Tone) {
        let label = clean(label);
        let value = clean(value);
        let label_width = console::measure_text_width(&label).max(16);
        if label_width + 3 + console::measure_text_width(&value) <= self.inner {
            let gap = label_width - console::measure_text_width(&label) + 3;
            self.line(format!(
                "{}{}{}",
                paint(label, Tone::Muted),
                " ".repeat(gap),
                paint(value, tone)
            ));
        } else {
            self.text(&label, Tone::Muted);
            self.text(&value, tone);
        }
    }

    pub fn finish(mut self) -> Vec<String> {
        self.line("");
        self.lines.push(format!(
            "  {}{}{}",
            paint("╰", Tone::Muted),
            paint("─".repeat(self.inner + 2), Tone::Muted),
            paint("╯", Tone::Muted)
        ));
        self.lines
    }
}

pub(crate) fn print_card(card: Card) {
    println!();
    for line in card.finish() {
        println!("{line}");
    }
    let _ = std::io::stdout().flush();
}

pub(crate) fn notice(title: &str, message: &str, tone: Tone) {
    let mut card = Card::new(title, dimensions().0, tone);
    card.text(message, Tone::Plain);
    print_card(card);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn card_columns_and_selection_labels_are_bounded_for_cjk() {
        for width in [29, 45, 76] {
            let mut card = Card::new("共享状态", width, Tone::Accent);
            card.field("设备名称", "办公室的电脑（共享与权限设置）。", Tone::Plain);
            let lines = card.finish();
            let expected = console::measure_text_width(&lines[0]);
            for line in lines {
                assert_eq!(console::measure_text_width(&line), expected, "{line}");
            }
            let clipped = fit("设置（共享配置），请确认。", 8);
            assert!(console::measure_text_width(&clipped) <= 8);
            assert!(!clipped.trim_end_matches('…').ends_with('（'));
        }
    }
}
