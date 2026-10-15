use crate::output::{Failure, command};
use anyhow::Context;
use removent_core::settings::control::{self, EditError, SETTINGS, Setting};
use removent_core::{DataPaths, Settings};
use rust_i18n::t;
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};

pub(super) struct Options {
    action: Action,
    json: bool,
}

enum Action {
    List,
    Get(Option<String>),
    Set {
        key: String,
        value: String,
        stdin: bool,
    },
    Reset(String),
    Path,
}

pub(super) fn parse(raw: &[String]) -> anyhow::Result<Options> {
    let mut json = false;
    let mut args = Vec::new();
    for arg in raw {
        // The third positional argument to set is a value, even if it looks
        // like a flag. Put --json before the action or after its value.
        if arg == "--json"
            && !(args.first().is_some_and(|v: &&String| v.as_str() == "set") && args.len() == 2)
        {
            json = true;
        } else {
            args.push(arg);
        }
    }
    let usage = || Failure::usage(t!("settings.invalid_arguments"), "settings");
    let action = match args.as_slice() {
        [] => Action::List,
        [action] if action.as_str() == "list" => Action::List,
        [action] if action.as_str() == "path" => Action::Path,
        [action] if action.as_str() == "get" => Action::Get(None),
        [action, key] if action.as_str() == "get" => Action::Get(Some((*key).clone())),
        [action, key] if action.as_str() == "reset" => Action::Reset((*key).clone()),
        [action, key, value] if action.as_str() == "set" => Action::Set {
            key: (*key).clone(),
            value: (*value).clone(),
            stdin: value.as_str() == "--stdin",
        },
        _ => return Err(usage().into()),
    };
    let key = match &action {
        Action::Get(Some(key)) | Action::Set { key, .. } | Action::Reset(key) => Some(key),
        _ => None,
    };
    if let Some(key) = key {
        let setting = control::find(key).map_err(edit_error)?;
        if let Action::Set { value, stdin, .. } = &action {
            if setting.secret && !stdin {
                return Err(
                    Failure::usage(t!("settings.secret_stdin", key = key), "settings").into(),
                );
            }
            if !stdin {
                setting.parse(value).map_err(edit_error)?;
            }
        }
    }
    Ok(Options { action, json })
}

pub(super) fn edit_error(error: EditError) -> anyhow::Error {
    match error {
        EditError::Unknown(key) => {
            Failure::usage(t!("settings.unknown", key = key), "settings").into()
        }
        EditError::Invalid { key, expected } => {
            let expected = match expected.as_str() {
                "a non-empty device name" => t!("settings.expected_name").into_owned(),
                "an HTTPS URL without credentials or a fragment, or empty text" => {
                    t!("settings.expected_endpoint").into_owned()
                }
                "a valid Base32 OTP secret" => t!("settings.expected_otp").into_owned(),
                "a configured password for password mode, or a valid OTP secret for OTP mode" => {
                    t!("settings.expected_auth").into_owned()
                }
                _ => control::find(key).map(accepted_text).unwrap_or(expected),
            };
            Failure::usage(
                t!("settings.invalid", key = key, expected = expected),
                "settings",
            )
            .into()
        }
        EditError::Storage(error) => anyhow::Error::new(error).context(Failure::runtime(
            t!("settings.save_failed"),
            t!("settings.file_hint", command = command("settings path")),
        )),
    }
}

impl Options {
    pub fn with_format(mut self, format: crate::terminal::Format) -> Self {
        if format != crate::terminal::Format::Auto {
            self.json = format == crate::terminal::Format::Json;
        }
        self
    }
}

pub(super) async fn run(options: Options, paths: &DataPaths) -> anyhow::Result<()> {
    match options.action {
        Action::Path => {
            if options.json {
                println!("{}", json!({"path": paths.settings_file()}));
            } else {
                println!("{}", paths.settings_file().display());
            }
        }
        Action::List | Action::Get(None) => {
            let current = load(paths)?;
            if options.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &SETTINGS
                            .iter()
                            .map(|setting| setting.view(&current))
                            .collect::<Vec<_>>()
                    )?
                );
            } else if crate::components::enabled() {
                use crate::components::{Card, Tone};
                let mut groups: Vec<(String, Vec<&Setting>)> = Vec::new();
                for setting in SETTINGS {
                    let name = crate::components::results::setting_group(setting.key);
                    if let Some((_, items)) = groups.iter_mut().find(|(group, _)| *group == name) {
                        items.push(setting);
                    } else {
                        groups.push((name, vec![setting]));
                    }
                }
                for (group, items) in groups {
                    let mut card =
                        Card::new(&group, crate::components::dimensions().0, Tone::Accent);
                    for setting in items {
                        card.field(
                            &crate::components::results::setting_label(setting.key),
                            &display_value(&setting.view(&current)),
                            Tone::Plain,
                        );
                        if matches!(options.action, Action::List) {
                            card.text(setting.key, Tone::Muted);
                        }
                    }
                    crate::components::print_card(card);
                }
            } else {
                for setting in SETTINGS {
                    crate::terminal::field(setting.key, &display_value(&setting.view(&current)));
                    if matches!(options.action, Action::List) {
                        crate::terminal::note(&accepted_text(setting));
                        crate::terminal::note(&if setting.host {
                            t!("settings.host_apply")
                        } else {
                            t!("settings.desktop_apply")
                        });
                    }
                }
            }
        }
        Action::Get(Some(key)) => {
            let current = load(paths)?;
            let view = control::find(&key).map_err(edit_error)?.view(&current);
            if options.json {
                println!("{}", serde_json::to_string_pretty(&view)?);
            } else if crate::components::enabled() {
                use crate::components::{Card, Tone};
                let mut card = Card::new(
                    &crate::components::results::setting_label(&key),
                    crate::components::dimensions().0,
                    Tone::Accent,
                );
                card.field(&key, &display_value(&view), Tone::Plain);
                crate::components::print_card(card);
            } else {
                println!("{}", crate::terminal::clean(&display_value(&view)));
            }
        }
        Action::Set { key, value, stdin } => {
            let value = if stdin { read_stdin()? } else { value };
            let next = control::edit(paths, &key, Some(&value)).map_err(edit_error)?;
            after_save(
                paths,
                control::find(&key).map_err(edit_error)?,
                &next,
                options.json,
            )
            .await?;
        }
        Action::Reset(key) => {
            let next = control::edit(paths, &key, None).map_err(edit_error)?;
            after_save(
                paths,
                control::find(&key).map_err(edit_error)?,
                &next,
                options.json,
            )
            .await?;
        }
    }
    Ok(())
}

pub(super) fn load(paths: &DataPaths) -> anyhow::Result<Settings> {
    Settings::load_strict(paths).with_context(|| {
        Failure::runtime(
            t!("settings.load_failed"),
            t!("settings.file_hint", command = command("settings path")),
        )
    })
}

fn read_stdin() -> anyhow::Result<String> {
    if std::io::stdin().is_terminal() {
        return Err(Failure::usage(t!("settings.stdin_pipe"), "settings").into());
    }
    let mut input = String::new();
    std::io::stdin()
        .take(4097)
        .read_to_string(&mut input)
        .context(Failure::usage(t!("settings.stdin_invalid"), "settings"))?;
    if input.len() > 4096 {
        return Err(Failure::usage(t!("settings.stdin_invalid"), "settings").into());
    }
    // Remove one line ending from a pipe/file, preserving meaningful spaces.
    if input.ends_with('\n') {
        input.pop();
        if input.ends_with('\r') {
            input.pop();
        }
    }
    Ok(input)
}

pub(super) fn display_value(view: &Value) -> String {
    if view["secret"] == true {
        return if view["value"]["configured"] == true {
            t!("settings.secret_set")
        } else {
            t!("settings.secret_empty")
        }
        .into();
    }
    match &view["value"] {
        Value::String(value) => value.clone(),
        value => value.to_string(),
    }
}

pub(super) fn accepted_text(setting: &Setting) -> String {
    match setting.kind {
        control::Kind::Text(max) => t!("settings.text_limit", max = max).into(),
        _ => setting.accepted(),
    }
}

pub(super) async fn after_save(
    paths: &DataPaths,
    setting: &Setting,
    settings: &Settings,
    json_output: bool,
) -> anyhow::Result<()> {
    let reload = if setting.host {
        crate::components::progress::run(
            &t!("terminal.working"),
            !json_output,
            crate::daemon::request(paths, &removent_core::ipc::IpcRequest::ReloadSettings),
        )
        .await
        .map(|_| ())
    } else {
        Ok(())
    };
    let offline = reload
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<std::io::Error>())
        .is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
        });
    let application = if !setting.host {
        "desktop_restart"
    } else if offline {
        "daemon_offline"
    } else if reload.is_err() {
        "reload_failed"
    } else {
        "reload_requested"
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"saved":true, "setting":setting.view(settings), "application":application})
            )?
        );
    } else if crate::components::enabled() {
        use crate::components::{Card, Tone};
        let mut card = Card::new(
            &t!("tui.saved"),
            crate::components::dimensions().0,
            Tone::Success,
        );
        card.field(
            &crate::components::results::setting_label(setting.key),
            &display_value(&setting.view(settings)),
            Tone::Plain,
        );
        card.divider();
        let message = match application {
            "desktop_restart" => t!("settings.desktop_apply"),
            "daemon_offline" => t!("settings.offline"),
            "reload_requested" => t!("settings.reload_requested"),
            _ => t!("settings.reload_failed"),
        };
        card.text(
            &message,
            if application == "reload_failed" {
                Tone::Warning
            } else {
                Tone::Muted
            },
        );
        if setting.key == "device_name" {
            card.text(&t!("settings.desktop_apply"), Tone::Muted);
        }
        crate::components::print_card(card);
    } else {
        println!(
            "{}",
            t!(
                "settings.saved",
                key = setting.key,
                value = crate::terminal::clean(&display_value(&setting.view(settings)))
            )
        );
        match application {
            "desktop_restart" => println!("{}", t!("settings.desktop_apply")),
            "daemon_offline" => println!("{}", t!("settings.offline")),
            "reload_requested" => println!("{}", t!("settings.reload_requested")),
            _ => {}
        }
        if setting.key == "device_name" {
            println!("{}", t!("settings.desktop_apply"));
        }
    }
    if !offline {
        reload.context(Failure::runtime(
            t!("settings.reload_failed"),
            t!("settings.retry_reload", command = command("daemon reload")),
        ))?;
    }
    Ok(())
}
