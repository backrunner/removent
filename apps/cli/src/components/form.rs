use super::{Card, Tone, dimensions, fit, paint, region};
use crate::terminal::clean;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use rust_i18n::t;

fn form(title: &str, value: &str, hint: &str) -> Vec<String> {
    let (width, height) = dimensions();
    if height < 12 {
        return vec![fit(title, width), fit(value, width), fit(hint, width)];
    }
    let mut card = Card::new(title, width, Tone::Accent);
    card.line(paint(fit(value, card.inner), Tone::Accent));
    card.divider();
    card.line(paint(fit(hint, card.inner), Tone::Muted));
    card.finish()
}

pub(crate) async fn input(
    title: &str,
    initial: &str,
    secret: bool,
) -> anyhow::Result<Option<String>> {
    input_validated(title, initial, secret, |_| None).await
}

pub(crate) async fn input_validated(
    title: &str,
    initial: &str,
    secret: bool,
    validate: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Option<String>> {
    // Raw mode (and therefore no echo) is established before the prompt is drawn.
    let mut region = region::Region::new()?;
    let mut value: Vec<char> = initial.chars().collect();
    let mut position = value.len();
    let mut error: Option<String> = None;
    loop {
        let display = if secret {
            // Fixed mask avoids disclosing the secret's length.
            if value.is_empty() {
                t!("tui.secret_empty").into_owned()
            } else {
                t!("tui.secret_entered").into_owned()
            }
        } else {
            let prefix: String = value[..position].iter().collect();
            let suffix: String = value[position..].iter().collect();
            // Show the insertion point without exposing raw terminal controls.
            let budget = dimensions().0.saturating_sub(6);
            let mut tail = String::new();
            for character in prefix.chars().rev() {
                if console::measure_text_width(&tail)
                    + console::measure_text_width(&character.to_string())
                    > budget / 2
                {
                    break;
                }
                tail.insert(0, character);
            }
            format!("{tail}▏{suffix}")
        };
        let hint = error.as_deref().unwrap_or_default();
        let mut lines = form(title, &display, &t!("tui.input_keys"));
        if !hint.is_empty() {
            let available = dimensions().0;
            lines.push(paint(format!("  {}", fit(hint, available)), Tone::Danger));
        }
        region.draw(&lines)?;
        match region.next().await? {
            Event::Key(key) => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(region::cancelled().into());
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Enter => {
                    let text: String = value.iter().collect();
                    error = validate(&text);
                    if error.is_none() {
                        return Ok(Some(text));
                    }
                }
                KeyCode::Left => position = position.saturating_sub(1),
                KeyCode::Right => position = (position + 1).min(value.len()),
                KeyCode::Home => position = 0,
                KeyCode::End => position = value.len(),
                KeyCode::Backspace if position > 0 => {
                    position -= 1;
                    value.remove(position);
                }
                KeyCode::Delete if position < value.len() => {
                    value.remove(position);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    value.clear();
                    position = 0;
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !character.is_control()
                        && value.len() < 4096 =>
                {
                    value.insert(position, character);
                    position += 1;
                }
                _ => {}
            },
            Event::Paste(text) => {
                for character in clean(&text)
                    .chars()
                    .take(4096usize.saturating_sub(value.len()))
                {
                    value.insert(position, character);
                    position += 1;
                }
            }
            _ => {}
        }
    }
}

pub(crate) async fn confirm(title: &str) -> anyhow::Result<bool> {
    let mut region = region::Region::new()?;
    let mut yes = false;
    loop {
        let yes_label = format!("{} {} ", if yes { "›" } else { " " }, t!("terminal.yes"));
        let no_label = format!("{} {} ", if !yes { "›" } else { " " }, t!("terminal.no"));
        let value = format!("[ {no_label}]   [ {yes_label}]");
        let mut card = Card::new(&t!("tui.confirm_title"), dimensions().0, Tone::Warning);
        card.text(title, Tone::Plain);
        card.line("");
        card.line(paint(fit(&value, card.inner), Tone::Accent));
        card.divider();
        card.text(&t!("tui.confirm_keys"), Tone::Muted);
        region.draw(&card.finish())?;
        if let Event::Key(key) = region.next().await? {
            match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Err(region::cancelled().into());
                }
                KeyCode::Esc => return Ok(false),
                KeyCode::Enter => return Ok(yes),
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => yes = !yes,
                KeyCode::Char('y' | 'Y') => yes = true,
                KeyCode::Char('n' | 'N') => yes = false,
                _ => {}
            }
        }
    }
}
