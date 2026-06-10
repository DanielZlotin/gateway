use crate::text::normalize_log_line_count;
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

const CLI_AFTER_HELP: &str = r#"Examples:
  gateway
  gateway bot
  gateway heartbeat
  gateway logs [lines]
  gateway uninstall
  gateway version
  gateway run --prompt "Summarize status"
  gateway run --chat 123456789 --prompt "Summarize status"
  gateway run --prompt-file ./prompt.txt
  printf '%s\n' "Summarize status" | gateway run"#;

const RUN_AFTER_HELP: &str = r#"Prompt input comes from --prompt, then --prompt-file, then stdin.
Each invocation starts a fresh Codex session.
Final text is always printed to stdout.
Non-empty, non-OK final text is sent to one Telegram chat.
Without --chat, Telegram output goes to the first configured private chat ID.
With --chat ID, Telegram output goes only to that configured chat."#;

#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    Execute(Mode),
    Help(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Bot,
    Heartbeat,
    Logs(usize),
    Run(RunArgs),
    Uninstall,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    pub chat: Option<i64>,
}

#[derive(Debug, Parser)]
#[command(
    name = "gateway",
    about = "Lean Rust Telegram-to-Codex gateway.",
    after_help = CLI_AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "Run the Telegram bot for allowed chats.")]
    Bot,
    #[command(about = "Run scheduled heartbeat work when due.")]
    Heartbeat,
    #[command(about = "Print recent gateway logs.")]
    Logs(LogsCli),
    #[command(
        about = "Execute one fresh Codex prompt from automation.",
        after_help = RUN_AFTER_HELP
    )]
    Run(RunCli),
    #[command(about = "Stop the LaunchAgent and remove its plist.")]
    Uninstall,
    #[command(about = "Print the running binary version.")]
    Version,
}

#[derive(Debug, Args)]
struct LogsCli {
    #[arg(
        value_name = "lines",
        help = "Number of recent lines to print (default 10, max 200)."
    )]
    lines: Option<usize>,
}

#[derive(Debug, Args)]
struct RunCli {
    #[arg(
        long,
        value_name = "PROMPT",
        help = "Read prompt text from this option before --prompt-file or stdin."
    )]
    prompt: Option<String>,
    #[arg(
        long,
        value_name = "PROMPT_FILE",
        help = "Read prompt text from this file when --prompt is omitted."
    )]
    prompt_file: Option<PathBuf>,
    #[arg(
        long,
        value_name = "MODEL",
        help = "Override the default model for this run."
    )]
    model: Option<String>,
    #[arg(
        long,
        value_name = "CHAT",
        help = "Send Telegram output to this configured chat ID."
    )]
    chat: Option<i64>,
}

pub fn parse_cli_from<I, T>(args: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return Ok(CliAction::Help(err.to_string()));
        }
        Err(err) => return Err(err.to_string()),
    };
    Ok(CliAction::Execute(mode_from_cli(cli)))
}

pub fn parse_args_from<I, T>(args: I) -> Result<Mode, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match parse_cli_from(args)? {
        CliAction::Execute(mode) => Ok(mode),
        CliAction::Help(help) => Err(help),
    }
}

fn mode_from_cli(cli: Cli) -> Mode {
    match cli.command {
        None | Some(Command::Bot) => Mode::Bot,
        Some(Command::Heartbeat) => Mode::Heartbeat,
        Some(Command::Logs(args)) => Mode::Logs(normalize_log_line_count(args.lines)),
        Some(Command::Run(args)) => Mode::Run(RunArgs {
            prompt: args.prompt,
            prompt_file: args.prompt_file,
            model: args.model,
            chat: args.chat,
        }),
        Some(Command::Uninstall) => Mode::Uninstall,
        Some(Command::Version) => Mode::Version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn help_from(args: &[&str]) -> String {
        match parse_cli_from(args.iter().copied()).unwrap() {
            CliAction::Help(help) => help,
            CliAction::Execute(mode) => panic!("expected help, got {mode:?}"),
        }
    }

    fn assert_contains_all(text: &str, expected: &[&str]) {
        for item in expected {
            assert!(text.contains(item), "missing {item:?} in:\n{text}");
        }
    }

    fn explicit_subcommands() -> Vec<String> {
        Cli::command()
            .get_subcommands()
            .map(|command| command.get_name().to_string())
            .filter(|name| name != "help")
            .collect()
    }

    #[test]
    fn defaults_to_bot_mode_when_no_subcommand_is_given() {
        let mode = parse_args_from(["gateway"]).unwrap();
        assert_eq!(mode, Mode::Bot);
    }

    #[test]
    fn parses_run_mode_with_prompt() {
        let mode = parse_args_from([
            "gateway",
            "run",
            "--prompt",
            "summarize",
            "--model",
            "gpt-test",
            "--chat",
            "77",
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Run(RunArgs {
                prompt: Some("summarize".to_string()),
                prompt_file: None,
                model: Some("gpt-test".to_string()),
                chat: Some(77),
            })
        );
    }

    #[test]
    fn parses_run_mode_with_prompt_file() {
        let mode = parse_args_from(["gateway", "run", "--prompt-file", "./prompt.txt"]).unwrap();

        assert_eq!(
            mode,
            Mode::Run(RunArgs {
                prompt: None,
                prompt_file: Some(PathBuf::from("./prompt.txt")),
                model: None,
                chat: None,
            })
        );
    }

    #[test]
    fn parses_heartbeat_mode() {
        let mode = parse_args_from(["gateway", "heartbeat"]).unwrap();
        assert_eq!(mode, Mode::Heartbeat);
    }

    #[test]
    fn top_level_help_documents_commands_and_examples() {
        let help = help_from(&["gateway", "-h"]);

        assert_contains_all(
            &help,
            &[
                "Lean Rust Telegram-to-Codex gateway.",
                "Usage: gateway [COMMAND]",
                "bot",
                "Run the Telegram bot for allowed chats.",
                "heartbeat",
                "Run scheduled heartbeat work when due.",
                "logs",
                "Print recent gateway logs.",
                "run",
                "Execute one fresh Codex prompt from automation.",
                "uninstall",
                "Stop the LaunchAgent and remove its plist.",
                "version",
                "Print the running binary version.",
                "gateway run --prompt \"Summarize status\"",
                "gateway heartbeat",
                "gateway run --chat 123456789 --prompt \"Summarize status\"",
                "gateway run --prompt-file ./prompt.txt",
                "printf '%s\\n' \"Summarize status\" | gateway run",
            ],
        );
        assert!(!explicit_subcommands().contains(&"config".to_string()));
    }

    #[test]
    fn documented_subcommand_help_is_available() {
        for args in [
            &["gateway", "help"][..],
            &["gateway", "bot", "-h"],
            &["gateway", "logs", "-h"],
            &["gateway", "run", "-h"],
            &["gateway", "uninstall", "-h"],
            &["gateway", "version", "-h"],
        ] {
            let help = help_from(args);
            assert!(help.contains("Usage:"), "missing usage in:\n{help}");
        }
    }

    #[test]
    fn top_level_help_examples_cover_executable_subcommands() {
        let help = help_from(&["gateway", "-h"]);
        let examples = help
            .split_once("Examples:")
            .map(|(_, examples)| examples)
            .expect("top-level help must include examples");

        for command in explicit_subcommands() {
            let example = format!("gateway {command}");
            assert!(
                examples.contains(&example),
                "missing {example:?} example in:\n{examples}"
            );

            let args = ["gateway", command.as_str(), "-h"];
            assert!(
                help_from(&args).contains("Usage:"),
                "missing subcommand help for {command}"
            );
        }
    }

    #[test]
    fn readme_cli_examples_match_top_level_help_examples() {
        let readme_examples = include_str!("../README.md")
            .split_once("## 🧰 CLI\n\n```zsh\n")
            .and_then(|(_, rest)| rest.split_once("\n```"))
            .map(|(block, _)| block.lines().map(str::to_string).collect::<Vec<String>>())
            .expect("README must include a CLI examples block");
        let help_examples = help_from(&["gateway", "-h"])
            .split_once("Examples:\n")
            .map(|(_, examples)| {
                examples
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(|line| line.trim_start().to_string())
                    .collect::<Vec<String>>()
            })
            .expect("top-level help must include examples");

        assert_eq!(readme_examples, help_examples);
    }

    #[test]
    fn readme_documents_telegram_env_vars_once() {
        let readme = include_str!("../README.md");

        assert_eq!(readme.matches("GATEWAY_TELEGRAM_TOKEN").count(), 1);
        assert_eq!(readme.matches("GATEWAY_TELEGRAM_CHAT_ID").count(), 1);
    }

    #[test]
    fn run_help_documents_prompt_sources_and_telegram_targeting() {
        let help = help_from(&["gateway", "run", "-h"]);

        assert_contains_all(
            &help,
            &[
                "Execute one fresh Codex prompt from automation.",
                "--prompt <PROMPT>",
                "Read prompt text from this option before --prompt-file or stdin.",
                "--prompt-file <PROMPT_FILE>",
                "Read prompt text from this file when --prompt is omitted.",
                "--model <MODEL>",
                "Override the default model for this run.",
                "--chat <CHAT>",
                "Send Telegram output to this configured chat ID.",
                "Final text is always printed to stdout.",
                "Non-empty, non-OK final text is sent to one Telegram chat.",
            ],
        );
    }

    #[test]
    fn removed_config_mode_is_rejected() {
        let err = parse_args_from(["gateway", "config"]).unwrap_err();
        assert!(err.contains("unrecognized subcommand 'config'"));
    }

    #[test]
    fn removed_paths_mode_is_rejected() {
        let err = parse_args_from(["gateway", "paths"]).unwrap_err();
        assert!(err.contains("unrecognized subcommand 'paths'"));
    }

    #[test]
    fn parses_uninstall_mode() {
        let mode = parse_args_from(["gateway", "uninstall"]).unwrap();
        assert_eq!(mode, Mode::Uninstall);
    }

    #[test]
    fn parses_version_mode() {
        let mode = parse_args_from(["gateway", "version"]).unwrap();
        assert_eq!(mode, Mode::Version);
    }

    #[test]
    fn parses_logs_mode_with_default_line_count() {
        let mode = parse_args_from(["gateway", "logs"]).unwrap();
        assert_eq!(mode, Mode::Logs(10));
    }

    #[test]
    fn parses_logs_mode_with_capped_line_count() {
        let mode = parse_args_from(["gateway", "logs", "999"]).unwrap();
        assert_eq!(mode, Mode::Logs(200));
    }
}
