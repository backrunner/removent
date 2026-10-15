//! User-facing errors stay concise; source chains are opt-in diagnostics.
use rust_i18n::t;
use std::fmt;

#[derive(Debug)]
pub(super) struct Failure {
    pub message: String,
    pub hint: String,
    pub exit_code: u8,
}

impl Failure {
    pub fn runtime(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: hint.into(),
            exit_code: 1,
        }
    }

    pub fn usage(message: impl Into<String>, topic: &str) -> Self {
        Self {
            message: message.into(),
            hint: t!(
                "error.help_hint",
                command = command(format!("{topic} --help").trim())
            )
            .into(),
            exit_code: 2,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

pub(super) fn command(arguments: &str) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| "removent-cli".into());
    format!("{} {arguments}", shell_quote(&exe.to_string_lossy()))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn interrupted(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
    })
}

pub(super) fn report(error: anyhow::Error, verbose: bool) -> std::process::ExitCode {
    if interrupted(&error) {
        return std::process::ExitCode::from(130);
    }
    let failure = error.downcast_ref::<Failure>();
    if crate::components::enabled() && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
        use crate::components::{Card, Tone};
        let mut card = Card::new(
            &t!("tui.error_title"),
            crate::components::dimensions().0,
            Tone::Danger,
        );
        card.text(&error.to_string(), Tone::Plain);
        card.divider();
        if failure.is_some() {
            card.text(&t!("tui.next_step"), Tone::Warning);
        } else {
            card.text(&t!("error.verbose_hint"), Tone::Muted);
        }
        for line in card.finish() {
            eprintln!("{line}");
        }
        // Recovery commands are one logical line, outside a bordered card, so
        // copy/paste preserves executable paths and shell quoting at any width.
        if let Some(failure) = failure {
            eprintln!("  {}", crate::terminal::clean(&failure.hint));
        }
    } else {
        eprintln!(
            "{}",
            t!(
                "error.message",
                message = crate::terminal::clean(&error.to_string())
            )
        );
        if let Some(failure) = failure {
            eprintln!("{}", crate::terminal::clean(&failure.hint));
        }
    }
    if verbose {
        for source in error.chain().skip(1) {
            eprintln!(
                "{}",
                t!(
                    "error.cause",
                    message = crate::terminal::clean(&source.to_string())
                )
            );
        }
    } else if failure.is_none_or(|failure| failure.exit_code != 2) {
        eprintln!("{}", t!("error.verbose_hint"));
    }
    std::process::ExitCode::from(failure.map_or(1, |failure| failure.exit_code))
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_paths_are_safe_to_copy_into_a_shell() {
        assert_eq!(
            super::shell_quote("/tmp/Removent's $(echo test)/cli"),
            "'/tmp/Removent'\\''s $(echo test)/cli'"
        );
    }
}
