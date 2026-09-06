//! Opt-in Clap command builders and dispatch for `self update` and `self channel`.
use crate::{CancellationToken, Result, UpdateOptions, Updater};
use ::clap::{Arg, ArgAction, ArgMatches, Command, builder::PossibleValuesParser};

/// Command customization; `--check` has no shorthand by default.
#[derive(Clone, Copy, Debug, Default)]
pub struct CommandOptions {
    /// Opt into a shorthand when the parent does not already use it.
    pub check_short: Option<char>,
}

/// Build the `self` parent and its two subcommands.
pub fn command(updater: &Updater<'_>) -> Command {
    command_with_options(updater, CommandOptions::default())
}

/// Build `self` with explicit flag customization.
pub fn command_with_options(updater: &Updater<'_>, options: CommandOptions) -> Command {
    Command::new("self")
        .about(format!("Manage the {} CLI itself", updater.name()))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(update_command(updater, options))
        .subcommand(channel_command(updater))
}

fn yes() -> Arg {
    Arg::new("yes")
        .long("yes")
        .short('y')
        .action(ArgAction::SetTrue)
        .help("Skip the confirmation prompt")
}

/// Build a standalone `update` command.
pub fn update_command(updater: &Updater<'_>, options: CommandOptions) -> Command {
    let mut check = Arg::new("check")
        .long("check")
        .action(ArgAction::SetTrue)
        .help("Check for updates without installing");
    if let Some(short) = options.check_short {
        check = check.short(short);
    }
    Command::new("update").about(format!("Update {} to the latest version", updater.name()))
        .long_about(format!("Update {} to the latest release on the current update channel.\n\nUse '{} self channel' to view or switch the channel.", updater.name(), updater.name()))
        .arg(check).arg(Arg::new("force").long("force").short('f').action(ArgAction::SetTrue).help("Reinstall even if already up to date"))
        .arg(yes())
}

/// Build a standalone `channel` command, deriving choices from configured specs.
pub fn channel_command(updater: &Updater<'_>) -> Command {
    Command::new("channel")
        .about("View or switch the update channel")
        .arg(Arg::new("name").value_parser(PossibleValuesParser::new(
            updater.channels().iter().map(|s| s.name.to_string()),
        )))
        .arg(yes())
}

/// Dispatch root matches containing `self`, or `self` matches containing
/// `update`/`channel`. Returns false when no matching subcommand was selected.
/// Parsing and help remain the caller's responsibility; output uses updater streams.
pub fn dispatch(
    updater: &mut Updater<'_>,
    matches: &ArgMatches,
    token: &CancellationToken,
) -> Result<bool> {
    let matches = match matches.subcommand() {
        Some(("self", matches)) => matches,
        _ => matches,
    };
    match matches.subcommand() {
        Some(("update", args)) => updater.update(
            token,
            UpdateOptions {
                check: args.get_flag("check"),
                force: args.get_flag("force"),
                yes: args.get_flag("yes"),
            },
        )?,
        Some(("channel", args)) => match args.get_one::<String>("name") {
            Some(name) => updater.switch_channel(token, name.as_str(), args.get_flag("yes"))?,
            None => updater.show_channel(token)?,
        },
        _ => return Ok(false),
    }
    Ok(true)
}
