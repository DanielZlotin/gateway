use crate::text::normalize_log_line_count;
use clap::{Args, Parser, Subcommand};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Bot,
    Logs(usize),
    Paths,
    Run(RunArgs),
    Uninstall,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunArgs {
    pub job: String,
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    pub new_session: bool,
}

#[derive(Debug, Parser)]
#[command(name = "gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Bot,
    Logs(LogsCli),
    Paths,
    Run(RunCli),
    Uninstall,
}

#[derive(Debug, Args)]
struct LogsCli {
    lines: Option<usize>,
}

#[derive(Debug, Args)]
struct RunCli {
    #[arg(long)]
    job: String,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    new: bool,
}

pub fn parse_args_from<I, T>(args: I) -> Result<Mode, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|err| err.to_string())?;
    Ok(match cli.command {
        None | Some(Command::Bot) => Mode::Bot,
        Some(Command::Logs(args)) => Mode::Logs(normalize_log_line_count(args.lines)),
        Some(Command::Paths) => Mode::Paths,
        Some(Command::Run(args)) => Mode::Run(RunArgs {
            job: args.job,
            prompt: args.prompt,
            prompt_file: args.prompt_file,
            model: args.model,
            new_session: args.new,
        }),
        Some(Command::Uninstall) => Mode::Uninstall,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "--job",
            "daily",
            "--prompt",
            "summarize",
            "--model",
            "gpt-test",
            "--new",
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Run(RunArgs {
                job: "daily".to_string(),
                prompt: Some("summarize".to_string()),
                prompt_file: None,
                model: Some("gpt-test".to_string()),
                new_session: true,
            })
        );
    }

    #[test]
    fn parses_paths_mode() {
        let mode = parse_args_from(["gateway", "paths"]).unwrap();
        assert_eq!(mode, Mode::Paths);
    }

    #[test]
    fn parses_uninstall_mode() {
        let mode = parse_args_from(["gateway", "uninstall"]).unwrap();
        assert_eq!(mode, Mode::Uninstall);
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

    #[test]
    fn run_mode_requires_job() {
        let err = parse_args_from(["gateway", "run", "--prompt", "hello"]).unwrap_err();
        assert!(err.contains("--job"));
    }
}
