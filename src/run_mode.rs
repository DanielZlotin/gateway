use crate::cli::RunArgs;
use crate::codex::{run_codex, CodexConfig};
use crate::config::Config;
use crate::session::{SessionKey, SessionStore};
use crate::telegram::TelegramClient;
use crate::text::split_telegram_message;
use std::fs;
use std::io::Read;

pub fn load_prompt<R: Read>(args: &RunArgs, mut stdin: R) -> Result<String, String> {
    let prompt = match (&args.prompt, &args.prompt_file) {
        (Some(prompt), _) => prompt.clone(),
        (None, Some(path)) => {
            fs::read_to_string(path).map_err(|err| format!("read prompt file: {err}"))?
        }
        (None, None) => {
            let mut text = String::new();
            stdin
                .read_to_string(&mut text)
                .map_err(|err| format!("read stdin: {err}"))?;
            text
        }
    };
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("prompt is empty".to_string());
    }
    Ok(prompt)
}

pub fn run(args: RunArgs, cfg: Config) -> Result<String, String> {
    let prompt = load_prompt(&args, std::io::stdin())?;
    let store = SessionStore::new(
        cfg.chat_state_dir.clone(),
        cfg.cron_state_dir.clone(),
        cfg.codex_model.clone(),
    );
    let key = SessionKey::Cron(args.job.clone());
    if args.new_session {
        store.reset(&key)?;
    }
    let state = store.load(&key);
    let model = args.model.as_deref().unwrap_or(&state.model);
    let output = run_codex(
        &CodexConfig {
            bin: cfg.codex_bin.clone(),
            home: cfg.codex_home.clone(),
            user_home: cfg.user_home.clone(),
            xdg_config_home: cfg.xdg_config_home.clone(),
            xdg_cache_home: cfg.xdg_cache_home.clone(),
            xdg_data_home: cfg.xdg_data_home.clone(),
            xdg_state_home: cfg.xdg_state_home.clone(),
            workdir: cfg.codex_workdir.clone(),
            path: cfg.path.clone(),
            default_model: cfg.codex_model.clone(),
        },
        &prompt,
        state.session_id.as_deref(),
        model,
        cfg.codex_timeout,
        &cfg.state_dir,
    )?;
    if let Some(session_id) = output.session_id.as_deref() {
        store.save_run(&key, state.generation, session_id)?;
    }
    if let Some(chat_id) = args.telegram_chat {
        let tg = TelegramClient::new(&cfg.bot_token);
        for part in split_telegram_message(&output.final_text) {
            tg.send_message(chat_id, &part, 0)?;
        }
    }
    Ok(output.final_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn base_args() -> RunArgs {
        RunArgs {
            job: "daily".to_string(),
            prompt: None,
            prompt_file: None,
            model: None,
            new_session: false,
            telegram_chat: None,
        }
    }

    #[test]
    fn prompt_argument_wins_over_stdin() {
        let mut args = base_args();
        args.prompt = Some("from arg".to_string());

        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from arg"
        );
    }

    #[test]
    fn prompt_file_wins_over_stdin() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "from file").unwrap();
        let mut args = base_args();
        args.prompt_file = Some(file.path().to_path_buf());

        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from file"
        );
    }

    #[test]
    fn stdin_is_used_when_no_prompt_source_is_given() {
        let args = base_args();
        assert_eq!(
            load_prompt(&args, Cursor::new("from stdin")).unwrap(),
            "from stdin"
        );
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let args = base_args();
        let err = load_prompt(&args, Cursor::new("   ")).unwrap_err();
        assert!(err.contains("prompt is empty"));
    }
}
