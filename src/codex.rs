use crate::anthropic_proxy::AnthropicProxy;
use crate::config::Config;
use crate::provider::Provider;
use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: PathBuf,
    pub workdir: PathBuf,
    pub default_model: String,
}

impl From<&Config> for CodexConfig {
    fn from(cfg: &Config) -> Self {
        Self {
            bin: PathBuf::from("codex"),
            workdir: cfg.codex_workdir.clone(),
            default_model: cfg.default_provider_model().model.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOutput {
    pub final_text: String,
    pub session_id: Option<String>,
}

pub struct CodexRun<'a> {
    pub prompt: &'a str,
    pub session_id: Option<&'a str>,
    pub provider: Provider,
    pub model: &'a str,
    pub image_paths: &'a [PathBuf],
    pub timeout: Duration,
    pub state_dir: &'a Path,
    pub cancel: Option<Arc<AtomicBool>>,
}

const GATEWAY_DEVELOPER_INSTRUCTIONS: &str = include_str!("../prompts/SYSTEM.md");

#[allow(clippy::too_many_arguments)]
pub fn codex_args(
    out_path: &Path,
    session_id: Option<&str>,
    provider: Provider,
    model: &str,
    default_model: &str,
    workdir: &Path,
    claude_proxy_base_url: Option<&str>,
    image_paths: &[PathBuf],
) -> Result<Vec<String>, String> {
    let model = if model.trim().is_empty() {
        default_model
    } else {
        model.trim()
    };
    let out = out_path.to_string_lossy().to_string();
    let workdir = workdir.to_string_lossy().to_string();
    let developer_instructions_config = format!(
        "developer_instructions={}",
        serde_json::to_string(GATEWAY_DEVELOPER_INSTRUCTIONS)
            .expect("gateway developer instructions should serialize")
    );
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        let mut args = strings(["--search", "exec", "resume", "--ephemeral"]);
        append_model_provider_config(&mut args, provider, claude_proxy_base_url)?;
        append_image_args(&mut args, image_paths);
        args.extend(strings([
            "-c",
            &developer_instructions_config,
            "--skip-git-repo-check",
            "--dangerously-bypass-approvals-and-sandbox",
            "-m",
            model,
            "--output-last-message",
            &out,
            session_id,
            "-",
        ]));
        return Ok(args);
    }

    let mut args = strings(["--search", "exec", "--color", "never"]);
    append_model_provider_config(&mut args, provider, claude_proxy_base_url)?;
    append_image_args(&mut args, image_paths);
    args.extend(strings([
        "-c",
        &developer_instructions_config,
        "--cd",
        &workdir,
        "--skip-git-repo-check",
        "--dangerously-bypass-approvals-and-sandbox",
        "-m",
        model,
        "--output-last-message",
        &out,
        "-",
    ]));
    Ok(args)
}

pub fn parse_codex_json(output: &str) -> CodexOutput {
    let mut result = CodexOutput {
        final_text: String::new(),
        session_id: None,
    };

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(event) = serde_json::from_str::<CodexEvent>(line) else {
            continue;
        };
        if event.event_type == "thread.started" {
            result.session_id = event.thread_id;
        } else if event.event_type == "item.completed" {
            if let Some(item) = event.item.filter(|item| item.item_type == "agent_message") {
                result.final_text = item.text.trim().to_string();
            }
        }
    }
    result
}

pub fn run_codex(
    cfg: &CodexConfig,
    prompt: &str,
    session_id: Option<&str>,
    provider: Provider,
    model: &str,
    timeout: Duration,
    state_dir: &Path,
) -> Result<CodexOutput, String> {
    run_codex_stream(
        cfg,
        CodexRun {
            prompt,
            session_id,
            provider,
            model,
            image_paths: &[],
            timeout,
            state_dir,
            cancel: None,
        },
        |_| {},
    )
}

pub fn run_codex_stream(
    cfg: &CodexConfig,
    run: CodexRun<'_>,
    mut on_stdout: impl FnMut(&str),
) -> Result<CodexOutput, String> {
    fs::create_dir_all(run.state_dir).map_err(|err| format!("create state dir: {err}"))?;
    let out_file = tempfile::NamedTempFile::new_in(run.state_dir).map_err(|err| err.to_string())?;
    let out_path = out_file.path().to_path_buf();
    let claude_proxy = if run.provider == Provider::Claude {
        Some(AnthropicProxy::start(run.timeout)?)
    } else {
        None
    };
    let args = codex_args(
        &out_path,
        run.session_id,
        run.provider,
        run.model,
        &cfg.default_model,
        &cfg.workdir,
        claude_proxy.as_ref().map(|proxy| proxy.base_url()),
        run.image_paths,
    )?;

    let mut child = Command::new(&cfg.bin)
        .args(args)
        .current_dir(&cfg.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("start codex: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "open codex stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "open codex stderr".to_string())?;
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
    let stdout_handle = thread::spawn(move || {
        let mut reader = stdout;
        let mut buf = [0; 512];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let _ = stdout_tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                }
                Err(_) => break,
            }
        }
    });
    let stderr_handle = thread::spawn(move || {
        let mut reader = stderr;
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    });

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "open codex stdin".to_string())?
        .write_all(run.prompt.as_bytes())
        .map_err(|err| format!("write codex stdin: {err}"))?;
    drop(child.stdin.take());

    let start = Instant::now();
    let mut stdout_text = String::new();
    loop {
        while let Ok(chunk) = stdout_rx.try_recv() {
            stdout_text.push_str(&chunk);
            on_stdout(&chunk);
        }
        if run
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::SeqCst))
        {
            let _ = child.kill();
            let _ = child.wait().map_err(|err| err.to_string())?;
            let _ = stdout_handle.join();
            let _ = stderr_handle.join().unwrap_or_default();
            return Err("codex cancelled".to_string());
        }
        if start.elapsed() > run.timeout {
            let _ = child.kill();
            let _ = child.wait().map_err(|err| err.to_string())?;
            let _ = stdout_handle.join();
            let stderr = stderr_handle.join().unwrap_or_default();
            let final_text = final_text_from_outputs(&out_path, stdout_text.as_bytes(), &stderr);
            return Err(format!(
                "codex timed out after {:?}\n\n{final_text}",
                run.timeout
            ));
        }
        if child.try_wait().map_err(|err| err.to_string())?.is_some() {
            let status = child.wait().map_err(|err| err.to_string())?;
            let _ = stdout_handle.join();
            while let Ok(chunk) = stdout_rx.try_recv() {
                stdout_text.push_str(&chunk);
                on_stdout(&chunk);
            }
            let stderr = stderr_handle.join().unwrap_or_default();
            let final_text = fs::read_to_string(&out_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            let final_text = if final_text.is_empty() {
                stdout_text.trim().to_string()
            } else {
                final_text
            };
            if status.success() {
                return Ok(CodexOutput {
                    final_text,
                    session_id: parse_session_id(&String::from_utf8_lossy(&stderr)),
                });
            }
            let stderr = String::from_utf8_lossy(&stderr);
            return Err([final_text.as_str(), stderr.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn parse_session_id(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        line.trim()
            .strip_prefix("session id:")
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    })
}

fn final_text_from_outputs(out_path: &Path, stdout: &[u8], stderr: &[u8]) -> String {
    let parsed = parse_codex_json(&String::from_utf8_lossy(stdout));
    let final_text = fs::read_to_string(out_path).unwrap_or_default();
    let stderr_text = String::from_utf8_lossy(stderr);
    [
        final_text.trim(),
        parsed.final_text.trim(),
        stderr_text.trim(),
    ]
    .into_iter()
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join("\n\n")
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn append_image_args(args: &mut Vec<String>, image_paths: &[PathBuf]) {
    for path in image_paths {
        args.push("-i".to_string());
        args.push(path.to_string_lossy().to_string());
    }
}

fn append_model_provider_config(
    args: &mut Vec<String>,
    provider: Provider,
    claude_proxy_base_url: Option<&str>,
) -> Result<(), String> {
    let values = match provider {
        Provider::Codex => return Ok(()),
        Provider::Claude => {
            let base_url = claude_proxy_base_url
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "Claude provider requires an Anthropic proxy".to_string())?;
            vec![
                "model_provider=\"anthropic-gateway\"".to_string(),
                "model_providers.anthropic-gateway.name=\"Anthropic Gateway\"".to_string(),
                format!(
                    "model_providers.anthropic-gateway.base_url={}",
                    serde_json::to_string(base_url).expect("Anthropic proxy URL should serialize")
                ),
                "model_providers.anthropic-gateway.env_key=\"ANTHROPIC_API_KEY\"".to_string(),
                "model_providers.anthropic-gateway.wire_api=\"responses\"".to_string(),
            ]
        }
        Provider::Openrouter => vec![
            "model_provider=\"openrouter\"".to_string(),
            "model_providers.openrouter.name=\"openrouter\"".to_string(),
            "model_providers.openrouter.base_url=\"https://openrouter.ai/api/v1\"".to_string(),
            "model_providers.openrouter.env_key=\"OPENROUTER_API_KEY\"".to_string(),
            "model_providers.openrouter.wire_api=\"responses\"".to_string(),
        ],
    };
    for value in values {
        args.push("-c".to_string());
        args.push(value);
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct CodexEvent {
    #[serde(rename = "type")]
    event_type: String,
    thread_id: Option<String>,
    item: Option<CodexItem>,
}

#[derive(Debug, Deserialize)]
struct CodexItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};
    use tempfile::tempdir;

    #[test]
    fn codex_args_use_yolo_flag_for_new_session() {
        let args = codex_args(
            Path::new("/tmp/out"),
            None,
            crate::provider::Provider::Codex,
            "",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &[],
        )
        .unwrap();
        let joined = args.join(" ");

        assert_eq!(args[0], "--search");
        assert_eq!(args[1], "exec");
        assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(joined.contains("--color never"));
        assert!(joined.contains("--cd /work"));
        assert!(!joined.contains("model_instructions_file"));
        assert!(joined.contains("-c developer_instructions=\"# 🌉 Gateway Runtime Instructions"));
        assert!(!joined.contains("--ask-for-approval"));
        assert!(!joined.contains("--sandbox"));
    }

    #[test]
    fn codex_args_resume_session() {
        let args = codex_args(
            Path::new("/tmp/out"),
            Some("session-123"),
            crate::provider::Provider::Codex,
            "gpt-test",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &[],
        )
        .unwrap();
        let joined = args.join(" ");

        assert_eq!(args[0], "--search");
        assert_eq!(args[1], "exec");
        assert_eq!(args[2], "resume");
        assert_eq!(args[3], "--ephemeral");
        assert!(joined.starts_with("--search exec resume --ephemeral -c developer_instructions=\""));
        assert!(joined.contains("# 🌉 Gateway Runtime Instructions"));
        assert!(joined.contains("--skip-git-repo-check"));
        assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(joined.contains("-m gpt-test"));
        assert!(joined.contains("--output-last-message /tmp/out session-123 -"));
    }

    #[test]
    fn codex_args_configure_openrouter_provider() {
        let args = codex_args(
            Path::new("/tmp/out"),
            None,
            crate::provider::Provider::Openrouter,
            "openai/gpt-5.5",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &[],
        )
        .unwrap();
        let joined = args.join(" ");

        assert!(joined.contains("model_provider=\"openrouter\""));
        assert!(joined.contains("model_providers.openrouter.name=\"openrouter\""));
        assert!(
            joined.contains("model_providers.openrouter.base_url=\"https://openrouter.ai/api/v1\"")
        );
        assert!(joined.contains("model_providers.openrouter.env_key=\"OPENROUTER_API_KEY\""));
        assert!(joined.contains("model_providers.openrouter.wire_api=\"responses\""));
        assert!(joined.contains("-m openai/gpt-5.5"));
    }

    #[test]
    fn codex_args_attach_images_to_new_and_resumed_prompts() {
        let image_paths = vec![PathBuf::from("/tmp/one.png"), PathBuf::from("/tmp/two.jpg")];
        let new_args = codex_args(
            Path::new("/tmp/out"),
            None,
            crate::provider::Provider::Codex,
            "gpt-test",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &image_paths,
        )
        .unwrap();
        let resume_args = codex_args(
            Path::new("/tmp/out"),
            Some("session-123"),
            crate::provider::Provider::Codex,
            "gpt-test",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &image_paths,
        )
        .unwrap();

        assert!(new_args
            .windows(2)
            .any(|args| args == ["-i", "/tmp/one.png"]));
        assert!(new_args
            .windows(2)
            .any(|args| args == ["-i", "/tmp/two.jpg"]));
        assert!(resume_args
            .windows(2)
            .any(|args| args == ["-i", "/tmp/one.png"]));
        assert!(resume_args
            .windows(2)
            .any(|args| args == ["-i", "/tmp/two.jpg"]));
    }

    #[test]
    fn codex_args_configure_claude_slot_through_anthropic_proxy() {
        let args = codex_args(
            Path::new("/tmp/out"),
            None,
            crate::provider::Provider::Claude,
            "claude-opus-4-8",
            "gpt-5.5",
            Path::new("/work"),
            Some("http://127.0.0.1:12345/v1"),
            &[],
        )
        .unwrap();
        let joined = args.join(" ");

        assert!(joined.contains("model_provider=\"anthropic-gateway\""));
        assert!(joined.contains("model_providers.anthropic-gateway.name=\"Anthropic Gateway\""));
        assert!(joined
            .contains("model_providers.anthropic-gateway.base_url=\"http://127.0.0.1:12345/v1\""));
        assert!(joined.contains("model_providers.anthropic-gateway.env_key=\"ANTHROPIC_API_KEY\""));
        assert!(joined.contains("model_providers.anthropic-gateway.wire_api=\"responses\""));
        assert!(!joined.contains("model_provider=\"openrouter\""));
        assert!(!joined.contains("OPENROUTER_API_KEY"));
        assert!(joined.contains("-m claude-opus-4-8"));
    }

    #[test]
    fn codex_args_reject_claude_without_anthropic_proxy() {
        let err = codex_args(
            Path::new("/tmp/out"),
            None,
            crate::provider::Provider::Claude,
            "claude-opus-4-8",
            "gpt-5.5",
            Path::new("/work"),
            None,
            &[],
        )
        .unwrap_err();

        assert!(err.contains("Claude provider requires an Anthropic proxy"));
    }

    #[test]
    fn developer_instructions_block_private_data_in_telegram() {
        assert!(GATEWAY_DEVELOPER_INSTRUCTIONS.contains("Telegram"));
        assert!(GATEWAY_DEVELOPER_INSTRUCTIONS.contains("environment variables"));
        assert!(GATEWAY_DEVELOPER_INSTRUCTIONS.contains("private keys"));
    }

    #[test]
    fn parse_codex_json_extracts_thread_and_last_agent_message() {
        let output = r#"{"type":"thread.started","thread_id":"session-123"}
{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}
{"type":"item.completed","item":{"type":"agent_message","text":"bye"}}"#;

        assert_eq!(
            parse_codex_json(output),
            CodexOutput {
                session_id: Some("session-123".to_string()),
                final_text: "bye".to_string(),
            }
        );
    }

    #[test]
    fn codex_config_is_built_from_gateway_config() {
        let cfg = Config {
            bot_token: "token".to_string(),
            telegram_chat_ids: vec![42],
            default_telegram_chat_id: 42,
            telegram_bots: vec![crate::config::TelegramBotConfig {
                bot_token: "token".to_string(),
                chat_ids: vec![42],
                offset_file: PathBuf::from("/state/gateway/telegram.offset"),
            }],
            xdg_config_home: PathBuf::from("/xdg/config"),
            xdg_cache_home: PathBuf::from("/xdg/cache"),
            xdg_data_home: PathBuf::from("/xdg/data"),
            xdg_state_home: PathBuf::from("/xdg/state"),
            gateway_config_file: PathBuf::from("/xdg/config/gateway/config.json"),
            codex_workdir: PathBuf::from("/work"),
            models: vec![crate::config::ProviderModel {
                provider: crate::provider::Provider::Codex,
                model: "gpt-default".to_string(),
                role: crate::config::ModelRole::Default,
            }],
            tts: None,
            state_dir: PathBuf::from("/state/gateway"),
            chat_state_dir: PathBuf::from("/state/gateway/chats"),
            offset_file: PathBuf::from("/state/gateway/telegram.offset"),
            gateway_log_file: PathBuf::from("/state/gateway/logs/gateway.log"),
            launchd_target: "gui/<uid>/ai.gateway".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(24 * 60 * 60),
        };

        let codex = CodexConfig::from(&cfg);

        assert_eq!(codex.bin, PathBuf::from("codex"));
        assert_eq!(codex.workdir, PathBuf::from("/work"));
        assert_eq!(codex.default_model, "gpt-default");
    }

    #[test]
    fn parse_codex_json_ignores_bad_lines_and_non_agent_items() {
        let output = r#"not json
{"type":"thread.started","thread_id":null}
{"type":"item.completed","item":{"type":"tool_call","text":"ignored"}}
{"type":"item.completed","item":null}
{"type":"item.completed","item":{"type":"agent_message","text":" final "}}"#;

        assert_eq!(
            parse_codex_json(output),
            CodexOutput {
                final_text: "final".to_string(),
                session_id: None,
            }
        );
    }

    #[test]
    fn run_codex_stream_uses_output_file_and_session_id() {
        let dir = tempdir().unwrap();
        let fake_codex = executable(
            dir.path().join("codex"),
            r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/tmp/gateway-codex-stdin
printf '{"type":"item.completed","item":{"type":"agent_message","text":"streamed"}}\n'
printf 'file final\n' > "$out"
printf 'session id: session-123\n' >&2
"#,
        );
        let cfg = codex_config(&fake_codex, dir.path());
        let mut streamed = String::new();

        let output = run_codex_stream(
            &cfg,
            CodexRun {
                prompt: "prompt",
                session_id: None,
                provider: Provider::Codex,
                model: "",
                image_paths: &[],
                timeout: Duration::from_secs(5),
                state_dir: &dir.path().join("state"),
                cancel: None,
            },
            |chunk| streamed.push_str(chunk),
        )
        .unwrap();

        assert_eq!(
            output,
            CodexOutput {
                final_text: "file final".to_string(),
                session_id: Some("session-123".to_string()),
            }
        );
        assert!(streamed.contains("streamed"));
    }

    #[test]
    fn run_codex_stream_inherits_parent_environment() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("GATEWAY_CODEX_ENV_TEST", "from-env");
        let dir = tempdir().unwrap();
        let fake_codex = executable(
            dir.path().join("codex"),
            &[
                r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
"#,
                r#"printf '%s\n' "${"#,
                r#"GATEWAY_CODEX_ENV_TEST:-missing"#,
                r#"}" > "$out"
"#,
            ]
            .concat(),
        );
        let cfg = codex_config(&fake_codex, dir.path());

        let output = run_codex_stream(
            &cfg,
            CodexRun {
                prompt: "prompt",
                session_id: None,
                provider: Provider::Codex,
                model: "",
                image_paths: &[],
                timeout: Duration::from_secs(5),
                state_dir: &dir.path().join("state"),
                cancel: None,
            },
            |_| {},
        )
        .unwrap();

        std::env::remove_var("GATEWAY_CODEX_ENV_TEST");
        assert_eq!(output.final_text, "from-env");
    }

    #[test]
    fn run_codex_falls_back_to_stdout_and_reports_process_failures() {
        let dir = tempdir().unwrap();
        let stdout_codex = executable(
            dir.path().join("codex-stdout"),
            r#"#!/bin/sh
cat >/dev/null
printf 'stdout final\n'
"#,
        );
        let cfg = codex_config(&stdout_codex, dir.path());
        let output = run_codex(
            &cfg,
            "prompt",
            Some("session-123"),
            Provider::Codex,
            "gpt-test",
            Duration::from_secs(5),
            &dir.path().join("state"),
        )
        .unwrap();
        assert_eq!(output.final_text, "stdout final");
        assert_eq!(output.session_id, None);

        let failing_codex = executable(
            dir.path().join("codex-fails"),
            r#"#!/bin/sh
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
printf 'file error\n' > "$out"
printf 'stderr error\n' >&2
exit 2
"#,
        );
        let cfg = codex_config(&failing_codex, dir.path());
        let err = run_codex(
            &cfg,
            "prompt",
            None,
            Provider::Codex,
            "gpt-test",
            Duration::from_secs(5),
            &dir.path().join("state"),
        )
        .unwrap_err();

        assert!(err.contains("file error"));
        assert!(err.contains("stderr error"));
    }

    #[test]
    fn run_codex_reports_start_and_timeout_errors() {
        let dir = tempdir().unwrap();
        let cfg = codex_config(&dir.path().join("missing-codex"), dir.path());
        let err = run_codex(
            &cfg,
            "prompt",
            None,
            Provider::Codex,
            "gpt-test",
            Duration::from_secs(5),
            &dir.path().join("state"),
        )
        .unwrap_err();
        assert!(err.contains("start codex"));

        let sleeping_codex = executable(
            dir.path().join("codex-sleeps"),
            r#"#!/bin/sh
printf '{"type":"item.completed","item":{"type":"agent_message","text":"partial"}}\n'
printf 'stderr partial\n' >&2
sleep 1
"#,
        );
        let cfg = codex_config(&sleeping_codex, dir.path());
        let err = run_codex(
            &cfg,
            "prompt",
            None,
            Provider::Codex,
            "gpt-test",
            Duration::from_millis(10),
            &dir.path().join("state"),
        )
        .unwrap_err();

        assert!(err.contains("codex timed out"));
    }

    #[test]
    fn run_codex_stream_kills_running_process_when_cancelled() {
        let dir = tempdir().unwrap();
        let fake_codex = executable(
            dir.path().join("codex-cancellable"),
            r#"#!/bin/sh
cat >/dev/null
printf 'ready\n'
sleep 5
"#,
        );
        let cfg = codex_config(&fake_codex, dir.path());
        let cancel = Arc::new(AtomicBool::new(false));

        let err = run_codex_stream(
            &cfg,
            CodexRun {
                prompt: "prompt",
                session_id: None,
                provider: Provider::Codex,
                model: "",
                image_paths: &[],
                timeout: Duration::from_secs(5),
                state_dir: &dir.path().join("state"),
                cancel: Some(cancel.clone()),
            },
            |chunk| {
                if chunk.contains("ready") {
                    cancel.store(true, Ordering::SeqCst);
                }
            },
        )
        .unwrap_err();

        assert_eq!(err, "codex cancelled");
    }

    #[test]
    fn final_text_from_outputs_joins_available_sources() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.txt");
        fs::write(&out, " file ").unwrap();
        let stdout =
            br#"{"type":"item.completed","item":{"type":"agent_message","text":"stdout"}}"#;

        assert_eq!(
            final_text_from_outputs(&out, stdout, b" stderr "),
            "file\n\nstdout\n\nstderr"
        );
        assert_eq!(parse_session_id("session id: \n"), None);
        assert_eq!(
            parse_session_id("session id: abc\n"),
            Some("abc".to_string())
        );
    }

    fn codex_config(bin: &Path, root: &Path) -> CodexConfig {
        CodexConfig {
            bin: bin.to_path_buf(),
            workdir: root.to_path_buf(),
            default_model: "gpt-default".to_string(),
        }
    }

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn executable(path: PathBuf, body: &str) -> PathBuf {
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
}
