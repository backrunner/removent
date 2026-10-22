use crate::{output::Failure, ping::PingArgs};
use rust_i18n::t;

pub(super) enum Command {
    Help(&'static str),
    Version,
    Identity,
    Ping(PingArgs),
    Daemon(String),
    PairingWatch,
    Settings(crate::settings::Options),
    Interactive,
}

pub(super) struct Options {
    pub verbose: bool,
    pub command: Command,
    pub format: crate::terminal::Format,
}

pub(super) fn parse(raw: &[String]) -> anyhow::Result<Options> {
    let mut verbose = false;
    let mut format = crate::terminal::Format::Auto;
    let mut args = Vec::new();
    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        if settings_value_is_next(&args) {
            args.push(arg.clone());
            continue;
        }
        if matches!(arg.as_str(), "--verbose" | "-v") {
            verbose = true;
        } else if matches!(arg.as_str(), "--json" | "--human") {
            let next = if arg == "--json" {
                crate::terminal::Format::Json
            } else {
                crate::terminal::Format::Human
            };
            if format != crate::terminal::Format::Auto && format != next {
                return Err(Failure::usage(t!("terminal.format_conflict"), "").into());
            }
            format = next;
        } else {
            args.push(arg.clone());
            // Flag-looking option values are still values, not global flags.
            if matches!(
                arg.as_str(),
                "--pin" | "--name" | "--count" | "--timeout" | "--relay-profile"
            ) && let Some(value) = it.next()
            {
                args.push(value.clone());
            }
        }
    }
    let topic = match args.first().map(String::as_str) {
        Some("daemon") => "daemon",
        Some("pairing") => "pairing",
        Some("ping") => "ping",
        Some("identity") => "identity",
        Some("settings") => "settings",
        Some("interactive") => "interactive",
        _ => "",
    };
    let command = match args.first().map(String::as_str) {
        None if format != crate::terminal::Format::Json && crate::interactive::available() => {
            Command::Interactive
        }
        None => Command::Help("usage"),
        Some("interactive") if args.len() == 1 && format != crate::terminal::Format::Json => {
            Command::Interactive
        }
        Some("--help" | "-h") if args.len() == 1 => Command::Help("usage"),
        Some("--version" | "-V") if args.len() == 1 => Command::Version,
        Some("daemon" | "pairing" | "ping" | "identity" | "settings" | "interactive")
            if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") =>
        {
            Command::Help(match topic {
                "daemon" => "help.daemon",
                "pairing" => "help.pairing",
                "ping" => "help.ping",
                "settings" => "help.settings",
                "interactive" => "help.interactive",
                _ => "help.identity",
            })
        }
        Some("identity") if args.len() == 1 => Command::Identity,
        Some("settings") => {
            Command::Settings(crate::settings::parse(&args[1..])?.with_format(format))
        }
        Some("ping") => Command::Ping(
            crate::ping::parse_args(&args[1..])
                .map_err(|error| Failure::usage(error.to_string(), topic))?,
        ),
        Some("daemon" | "pairing") => {
            if let Some(extra) = args.get(2) {
                return Err(Failure::usage(t!("args.unrecognized", arg = extra), topic).into());
            }
            let action = args
                .get(1)
                .map(String::as_str)
                .unwrap_or(if topic == "daemon" { "status" } else { "show" });
            if topic == "pairing" {
                match action {
                    "watch" => Command::PairingWatch,
                    "show" | "generate" | "revoke" => Command::Daemon(format!("pairing-{action}")),
                    _ => {
                        return Err(Failure::usage(
                            t!("pairing.unknown_action", action = action),
                            topic,
                        )
                        .into());
                    }
                }
            } else {
                if !matches!(
                    action,
                    "status" | "enable" | "disable" | "permissions" | "reload"
                ) && !(cfg!(target_os = "macos")
                    && matches!(
                        action,
                        "start"
                            | "ensure"
                            | "stop"
                            | "restart"
                            | "login-on"
                            | "login-off"
                            | "service-status"
                            | "install-defaults"
                            | "system-status"
                            | "system-enable"
                            | "system-update"
                            | "system-disable"
                    ))
                {
                    return Err(Failure::usage(
                        t!("daemon.unknown_action", action = action),
                        topic,
                    )
                    .into());
                }
                Command::Daemon(action.to_owned())
            }
        }
        Some("identity") => {
            return Err(Failure::usage(t!("args.unrecognized", arg = args[1]), topic).into());
        }
        Some(other) => {
            return Err(Failure::usage(t!("error.unknown_subcommand", cmd = other), topic).into());
        }
    };
    Ok(Options {
        verbose,
        command,
        format,
    })
}

fn settings_value_is_next(args: &[String]) -> bool {
    if args.first().is_none_or(|arg| arg != "settings") {
        return false;
    }
    let mut position = 0;
    for arg in &args[1..] {
        if arg == "--json" && position != 2 {
            continue;
        }
        match position {
            0 if arg == "set" => position = 1,
            1 => position = 2,
            _ => return false,
        }
    }
    position == 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_flags_do_not_consume_ping_option_values() {
        for (option, value) in [("--name", "--verbose"), ("--pin", "-v")] {
            let args = ["ping", "127.0.0.1:48688", option, value].map(str::to_owned);
            let parsed = parse(&args).unwrap();
            assert!(!parsed.verbose);
            let Command::Ping(ping) = parsed.command else {
                panic!("expected ping")
            };
            if option == "--name" {
                assert_eq!(ping.name, value);
            } else {
                assert_eq!(ping.pin.as_deref(), Some(value));
            }
        }
    }
}
