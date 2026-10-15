//! Inline keyboard menus: only the current prompt is redrawn, never scrollback.
use crate::components::{self, Item, Tone};
use crate::{daemon, output, settings, terminal};
use removent_core::{
    DataPaths,
    settings::control::{self, Kind, SETTINGS},
};
use rust_i18n::t;
use std::io::IsTerminal;
use terminal::Format;

pub(super) fn available() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
        && std::env::var("TERM").as_deref() != Ok("dumb")
}

pub(super) async fn select(prompt: &str, items: &[String]) -> anyhow::Result<Option<usize>> {
    let items: Vec<_> = items
        .iter()
        .map(|item| crate::components::Item::new(item.clone()))
        .collect();
    crate::components::choose(prompt, &items, 0).await
}

async fn confirm(prompt: &str) -> anyhow::Result<bool> {
    crate::components::confirm(prompt).await
}

async fn input(prompt: &str) -> anyhow::Result<Option<String>> {
    crate::components::input(prompt, "", false).await
}

pub(super) async fn run(paths: &DataPaths, verbose: bool) -> anyhow::Result<()> {
    if !available() {
        return Err(output::Failure::usage(t!("terminal.tty_required"), "").into());
    }
    // Handle SIGINT even between rendering a prompt and entering raw input.
    // The task restores the pre-menu terminal before exiting, including when a
    // synchronous prompt is blocked on another runtime thread.
    let _interrupt = crate::prompt::interrupt_guard()?;
    let _session = components::Session::new()?;
    components::results::welcome(&settings::load(paths)?.device_name);
    loop {
        let items = [
            "status", "service", "pairing", "settings", "identity", "ping", "exit",
        ]
        .map(|key| {
            let key = format!("terminal.menu.{key}");
            let detail_key = format!("{key}_detail");
            Item::new(t!(&key).into_owned()).detail(t!(&detail_key).into_owned())
        });
        let Some(index) = components::choose(&t!("terminal.choose"), &items, 0).await? else {
            return Ok(());
        };
        let result = match index {
            0 => daemon::cmd_daemon("status", Format::Human).await,
            1 => service().await,
            2 => pairing().await,
            3 => edit_settings(paths).await,
            4 => {
                let current = settings::load(paths)?;
                let identity =
                    removent_core::identity::load_or_create(paths, &current.device_name)?;
                terminal::render(
                    "identity",
                    &serde_json::json!({
                        "certificate_fingerprint": identity.fingerprint_hex(),
                        "public_key": hex::encode(identity.verifying_key().as_bytes()),
                    }),
                    Format::Human,
                )
            }
            5 => ping().await,
            _ => return Ok(()),
        };
        if let Err(error) = result {
            if output::interrupted(&error) {
                return Err(error);
            }
            output::report(error, verbose);
        }
        // Language edits take effect on the next prompt in this process too.
        if let Ok(current) = settings::load(paths) {
            rust_i18n::set_locale(removent_core::resolve_locale(current.language));
        }
    }
}

async fn service() -> anyhow::Result<()> {
    let mut actions = vec!["status", "enable", "disable", "permissions", "reload"];
    if cfg!(target_os = "macos") {
        actions.extend([
            "start",
            "stop",
            "restart",
            "ensure",
            "login-on",
            "login-off",
            "service-status",
        ]);
    }
    let labels: Vec<_> = actions
        .iter()
        .map(|key| {
            let key = format!("terminal.service.{key}");
            t!(&key).into_owned()
        })
        .collect();
    if let Some(index) = select(&t!("terminal.menu.service"), &labels).await? {
        let action = actions[index];
        if matches!(action, "disable" | "stop" | "restart" | "reload")
            && !confirm(&t!("terminal.interrupt_confirm")).await?
        {
            return Ok(());
        }
        daemon::cmd_daemon(action, Format::Human).await?;
    }
    Ok(())
}

async fn pairing() -> anyhow::Result<()> {
    let actions = ["show", "generate", "watch", "revoke"];
    let labels = actions.map(|key| {
        let key = format!("terminal.pairing.{key}");
        t!(&key).into_owned()
    });
    if let Some(index) = select(&t!("terminal.menu.pairing"), &labels).await? {
        let action = actions[index];
        if matches!(action, "generate" | "revoke")
            && !confirm(&t!("terminal.pairing_confirm")).await?
        {
            return Ok(());
        }
        if action == "watch" {
            daemon::watch_pairing(Format::Human).await?;
        } else {
            daemon::cmd_daemon(&format!("pairing-{action}"), Format::Human).await?;
        }
    }
    Ok(())
}

async fn edit_settings(paths: &DataPaths) -> anyhow::Result<()> {
    let current = settings::load(paths)?;
    let items: Vec<_> = SETTINGS
        .iter()
        .map(|setting| {
            Item::new(components::results::setting_label(setting.key))
                .value(components::results::setting_value(&setting.view(&current)))
                .detail(format!(
                    "{}: {}",
                    setting.key,
                    settings::accepted_text(setting)
                ))
                .group(components::results::setting_group(setting.key))
        })
        .collect();
    let Some(index) = components::choose(&t!("terminal.menu.settings"), &items, 0).await? else {
        return Ok(());
    };
    let setting = &SETTINGS[index];
    let mut card = components::Card::new(
        &components::results::setting_label(setting.key),
        components::dimensions().0,
        Tone::Accent,
    );
    card.field(
        setting.key,
        &settings::display_value(&setting.view(&current)),
        Tone::Plain,
    );
    card.text(&settings::accepted_text(setting), Tone::Muted);
    card.text(
        &if setting.host {
            t!("settings.host_apply")
        } else {
            t!("settings.desktop_apply")
        },
        Tone::Warning,
    );
    components::print_card(card);
    let actions = [
        t!("terminal.edit").into_owned(),
        t!("terminal.reset").into_owned(),
    ];
    let Some(action) = select(setting.key, &actions).await? else {
        return Ok(());
    };
    let value = if action == 1 {
        None
    } else if setting.secret {
        loop {
            let Some(value) = components::input(&t!("terminal.secret"), "", true).await? else {
                return Ok(());
            };
            let Some(repeated) = components::input(&t!("terminal.repeat"), "", true).await? else {
                return Ok(());
            };
            if value == repeated {
                break Some(value);
            }
            components::notice(
                &t!("tui.check_input"),
                &t!("terminal.mismatch"),
                Tone::Warning,
            );
        }
    } else {
        match setting.kind {
            Kind::Bool => {
                let choices = [
                    t!("terminal.yes").into_owned(),
                    t!("terminal.no").into_owned(),
                ];
                let choices: Vec<_> = choices.into_iter().map(Item::new).collect();
                let default = usize::from(setting.view(&current)["value"] != true);
                let Some(index) = components::choose(setting.key, &choices, default).await? else {
                    return Ok(());
                };
                Some((index == 0).to_string())
            }
            Kind::Choice(choices) => {
                let default = choices
                    .iter()
                    .position(|value| {
                        value
                            .eq_ignore_ascii_case(&settings::display_value(&setting.view(&current)))
                    })
                    .unwrap_or(0);
                let items: Vec<_> = choices
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        Item::new(components::results::choice_label(value))
                            .detail(*value)
                            .value(if index == default {
                                t!("tui.current").into_owned()
                            } else {
                                String::new()
                            })
                    })
                    .collect();
                let Some(index) = components::choose(setting.key, &items, default).await? else {
                    return Ok(());
                };
                Some(choices[index].to_string())
            }
            _ => match components::input_validated(
                &t!("terminal.value"),
                &settings::display_value(&setting.view(&current)),
                false,
                |value| {
                    setting
                        .parse(value)
                        .err()
                        .map(settings::edit_error)
                        .map(|error| error.to_string())
                },
            )
            .await?
            {
                Some(value) => Some(value),
                None => return Ok(()),
            },
        }
    };
    if let Some(value) = &value {
        setting.parse(value).map_err(settings::edit_error)?;
    }
    let mut review =
        components::Card::new(&t!("tui.review"), components::dimensions().0, Tone::Accent);
    review.text(
        &components::results::setting_label(setting.key),
        Tone::Plain,
    );
    review.field(
        &t!("tui.previous"),
        &settings::display_value(&setting.view(&current)),
        Tone::Muted,
    );
    let preview = match &value {
        None => settings::display_value(&setting.view(&removent_core::Settings::default())),
        Some(value) if setting.secret => {
            if value.is_empty() {
                t!("settings.secret_empty").into_owned()
            } else {
                t!("settings.secret_set").into_owned()
            }
        }
        Some(value) => value.clone(),
    };
    review.field(&t!("tui.new_value"), &preview, Tone::Accent);
    components::print_card(review);
    if !confirm(&t!("terminal.save_confirm")).await? {
        return Ok(());
    }
    let next = control::edit(paths, setting.key, value.as_deref()).map_err(settings::edit_error)?;
    settings::after_save(paths, setting, &next, false).await
}

async fn ping() -> anyhow::Result<()> {
    terminal::note(&t!("terminal.ping_support"));
    let Some(target) = input(&t!("terminal.target")).await? else {
        return Ok(());
    };
    if target.is_empty() {
        return Ok(());
    }
    let args = crate::ping::parse_args(&[target])?;
    crate::ping::cmd_ping(args, Format::Human).await
}
