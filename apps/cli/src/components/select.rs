use super::{Card, Tone, dimensions, fit, paint, region};
use crate::terminal::clean;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use rust_i18n::t;

#[derive(Clone)]
pub(crate) struct Item {
    pub label: String,
    pub detail: String,
    pub value: String,
    pub group: String,
}

impl Item {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            value: String::new(),
            group: String::new(),
        }
    }
    pub fn detail(mut self, value: impl Into<String>) -> Self {
        self.detail = value.into();
        self
    }
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }
    pub fn group(mut self, value: impl Into<String>) -> Self {
        self.group = value.into();
        self
    }
}

fn filtered(items: &[Item], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            format!("{} {} {}", item.label, item.detail, item.group)
                .to_lowercase()
                .contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

fn row(item: &Item, selected: bool, width: usize) -> String {
    let prefix = if selected { "› " } else { "  " };
    let available = width.saturating_sub(2);
    let value_width = console::measure_text_width(&item.value).min(available / 3);
    let label_width = available.saturating_sub(if item.value.is_empty() {
        0
    } else {
        value_width + 2
    });
    let label = fit(&item.label, label_width);
    let value = fit(&item.value, value_width);
    let gap = available
        .saturating_sub(console::measure_text_width(&label) + console::measure_text_width(&value));
    paint(
        format!("{prefix}{label}{}{value}", " ".repeat(gap)),
        if selected { Tone::Accent } else { Tone::Plain },
    )
}

fn view(
    title: &str,
    items: &[Item],
    matches: &[usize],
    selected: usize,
    query: &str,
) -> Vec<String> {
    let (width, height) = dimensions();
    if height < 14 {
        return vec![
            paint(fit(title, width), Tone::Accent),
            matches
                .get(selected)
                .map(|&i| row(&items[i], true, width))
                .unwrap_or_else(|| t!("tui.no_matches").into()),
            fit(&t!("tui.keys_compact"), width),
        ];
    }
    let mut card = Card::new(title, width, Tone::Accent);
    let search = if query.is_empty() {
        t!("tui.search").into_owned()
    } else {
        format!("/ {query}")
    };
    card.line(paint(fit(&search, card.inner), Tone::Muted));
    card.divider();
    let capacity = height.saturating_sub(11).clamp(1, 7);
    let start = selected.saturating_sub(capacity - 1);
    for position in start..(start + capacity).min(matches.len()) {
        card.line(row(
            &items[matches[position]],
            position == selected,
            card.inner,
        ));
    }
    if matches.is_empty() {
        card.line(paint(fit(&t!("tui.no_matches"), card.inner), Tone::Warning));
    }
    card.divider();
    if let Some(&index) = matches.get(selected) {
        let item = &items[index];
        let detail = if item.group.is_empty() {
            item.detail.clone()
        } else {
            format!("[ {} ]  {}", item.group, item.detail)
        };
        card.line(paint(fit(&detail, card.inner), Tone::Muted));
    } else {
        card.line("");
    }
    let footer_key = if card.inner < 45 {
        "tui.keys_short"
    } else {
        "tui.keys"
    };
    let footer = t!(
        footer_key,
        current = if matches.is_empty() { 0 } else { selected + 1 },
        total = matches.len()
    );
    card.line(paint(fit(&footer, card.inner), Tone::Muted));
    card.finish()
}

pub(crate) async fn choose(
    title: &str,
    items: &[Item],
    default: usize,
) -> anyhow::Result<Option<usize>> {
    let mut region = region::Region::new()?;
    let mut query = String::new();
    let mut selected = default.min(items.len().saturating_sub(1));
    loop {
        let matches = filtered(items, &query);
        selected = selected.min(matches.len().saturating_sub(1));
        region.draw(&view(title, items, &matches, selected, &query))?;
        match region.next().await? {
            Event::Key(key) => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(region::cancelled().into());
                }
                KeyCode::Esc if !query.is_empty() => {
                    query.clear();
                    selected = 0;
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('q') if query.is_empty() => return Ok(None),
                KeyCode::Enter => {
                    if let Some(&index) = matches.get(selected) {
                        drop(region);
                        // One compact breadcrumb replaces the live menu.
                        println!(
                            "  {} {}",
                            paint("›", Tone::Accent),
                            clean(&items[index].label)
                        );
                        return Ok(Some(index));
                    }
                }
                KeyCode::Up | KeyCode::BackTab if !matches.is_empty() => {
                    selected = (selected + matches.len() - 1) % matches.len()
                }
                KeyCode::Down | KeyCode::Tab if !matches.is_empty() => {
                    selected = (selected + 1) % matches.len()
                }
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = matches.len().saturating_sub(1),
                KeyCode::PageDown => selected = (selected + 7).min(matches.len().saturating_sub(1)),
                KeyCode::PageUp => selected = selected.saturating_sub(7),
                KeyCode::Backspace => {
                    query.pop();
                    selected = 0;
                }
                KeyCode::Char(value)
                    if !key.modifiers.contains(KeyModifiers::CONTROL) && query.len() < 128 =>
                {
                    query.push(value);
                    selected = 0;
                }
                _ => {}
            },
            Event::Paste(value) => {
                query = clean(&value).chars().take(128).collect();
                selected = 0;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtering_preserves_the_original_action_and_focus_has_fixed_geometry() {
        let items = [
            Item::new("Theme").group("Appearance"),
            Item::new("设备名称")
                .detail("device_name")
                .value("办公室电脑"),
        ];
        assert_eq!(filtered(&items, "device_name"), vec![1]);
        assert_eq!(filtered(&items, "appearance"), vec![0]);
        assert!(filtered(&items, "not-present").is_empty());
        for width in [20, 40, 70] {
            assert_eq!(
                console::measure_text_width(&row(&items[1], false, width)),
                width
            );
            assert_eq!(
                console::measure_text_width(&row(&items[1], true, width)),
                width
            );
        }
    }
}
