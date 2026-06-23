use gateway::cli::{parse_cli_from, CliAction, Mode};

fn main() {
    if let Err(err) = run() {
        gateway::logs::error(format_args!("{err}"));
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mode = match parse_cli_from(std::env::args_os())? {
        CliAction::Execute(mode) => mode,
        CliAction::Help(help) => {
            print!("{help}");
            return Ok(());
        }
    };

    match mode {
        Mode::Bot => gateway::bot::run(load_config_with_context(&Mode::Bot)?),
        Mode::Heartbeat => print_output(gateway::heartbeat::run(load_config_with_context(
            &Mode::Heartbeat,
        )?)),
        Mode::List(args) => {
            print_output(gateway::cli_commands::list(args, gateway::config::load()?))
        }
        Mode::Logs(lines) => print_output(gateway::logs::read_gateway_logs(
            &gateway::config::current_env(),
            lines,
        )),
        selected @ Mode::Run(_) => {
            let cfg = load_config_with_context(&selected)?;
            let args = match selected {
                Mode::Run(args) => args,
                _ => return Err("internal gateway mode mismatch".to_string()),
            };
            print_output(gateway::run_mode::run(args, cfg))
        }
        selected @ Mode::Status(_) => {
            let cfg = load_config_with_context(&selected)?;
            let args = match selected {
                Mode::Status(args) => args,
                _ => return Err("internal gateway mode mismatch".to_string()),
            };
            print_output(gateway::cli_commands::status(args, cfg))
        }
        Mode::Update => print_output(gateway::cli_commands::update(gateway::config::load()?)),
        Mode::Uninstall => print_output(gateway::launchd::uninstall()),
        Mode::Version => print_output(Ok(format!("gateway {}", env!("CARGO_PKG_VERSION")))),
    }
}

fn load_config_with_context(mode: &Mode) -> Result<gateway::config::Config, String> {
    let cfg = gateway::config::load()?;
    ensure_context_for_mode(mode, &cfg)?;
    Ok(cfg)
}

fn ensure_context_for_mode(mode: &Mode, cfg: &gateway::config::Config) -> Result<(), String> {
    match mode {
        Mode::Bot | Mode::Heartbeat | Mode::Run(_) | Mode::Status(_) => {
            gateway::context::ensure_gateway_context_files(&cfg.xdg_config_home)
        }
        Mode::List(_) | Mode::Logs(_) | Mode::Update | Mode::Uninstall | Mode::Version => Ok(()),
    }
}

fn print_output(output: Result<String, String>) -> Result<(), String> {
    println!("{}", output?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway::cli::{ChatArgs, RunArgs};
    use gateway::config::{Config, ModelRole, ProviderModel, TelegramBotConfig};
    use gateway::provider::Provider;
    use std::path::Path;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn ensure_context_for_codex_modes_creates_context_files() {
        let modes = [
            Mode::Bot,
            Mode::Heartbeat,
            Mode::Run(RunArgs {
                prompt: Some("status".to_string()),
                prompt_file: None,
                model: None,
                chat: None,
            }),
            Mode::Status(ChatArgs { chat: Some(42) }),
        ];

        for mode in modes {
            let dir = tempdir().unwrap();
            let cfg = test_config(dir.path());

            ensure_context_for_mode(&mode, &cfg).unwrap();

            assert_context_files_exist(&cfg, &["AGENTS.md", "MEMORY.md", "HEARTBEAT.md"]);
        }
    }

    #[test]
    fn ensure_context_for_non_codex_modes_does_not_create_context_files() {
        let modes = [
            Mode::List(ChatArgs { chat: Some(42) }),
            Mode::Logs(10),
            Mode::Update,
            Mode::Uninstall,
            Mode::Version,
        ];

        for mode in modes {
            let dir = tempdir().unwrap();
            let cfg = test_config(dir.path());

            ensure_context_for_mode(&mode, &cfg).unwrap();

            assert!(!cfg.xdg_config_home.join("gateway").exists());
        }
    }

    fn assert_context_files_exist(cfg: &Config, names: &[&str]) {
        for name in names {
            assert!(
                cfg.xdg_config_home.join("gateway").join(name).exists(),
                "missing {name}"
            );
        }
    }

    fn test_config(root: &Path) -> Config {
        let xdg_config_home = root.join("config");
        let state_dir = root.join("state/gateway");
        Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            default_telegram_chat_id: 42,
            telegram_bots: vec![TelegramBotConfig {
                bot_token: "token".to_string(),
                chat_ids: vec![42],
                offset_file: state_dir.join("telegram.offset"),
            }],
            xdg_config_home: xdg_config_home.clone(),
            xdg_cache_home: root.join("cache"),
            xdg_data_home: root.join("data"),
            xdg_state_home: root.join("state"),
            gateway_config_file: xdg_config_home.join("gateway/config.json"),
            codex_workdir: root.to_path_buf(),
            models: vec![ProviderModel {
                provider: Provider::Codex,
                model: "gpt-test".to_string(),
                role: ModelRole::Default,
            }],
            tts: None,
            state_dir: state_dir.clone(),
            chat_state_dir: state_dir.join("chats"),
            offset_file: state_dir.join("telegram.offset"),
            gateway_log_file: state_dir.join("logs/gateway.log"),
            launchd_target: "gui/123/ai.gateway".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(60),
        }
    }
}
