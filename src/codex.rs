use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: PathBuf,
    pub home: PathBuf,
    pub user_home: PathBuf,
    pub xdg_config_home: PathBuf,
    pub xdg_cache_home: PathBuf,
    pub xdg_data_home: PathBuf,
    pub xdg_state_home: PathBuf,
    pub workdir: PathBuf,
    pub path: String,
    pub default_model: String,
    pub instructions_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOutput {
    pub final_text: String,
    pub session_id: Option<String>,
}

pub fn codex_args(
    out_path: &Path,
    session_id: Option<&str>,
    model: &str,
    default_model: &str,
    workdir: &Path,
    instructions_path: &Path,
) -> Vec<String> {
    let model = if model.trim().is_empty() {
        default_model
    } else {
        model.trim()
    };
    let out = out_path.to_string_lossy().to_string();
    let workdir = workdir.to_string_lossy().to_string();
    let instructions_config = format!(
        "model_instructions_file={:?}",
        instructions_path.to_string_lossy()
    );
    if let Some(session_id) = session_id.filter(|value| !value.trim().is_empty()) {
        return strings([
            "exec",
            "resume",
            "--json",
            "-c",
            &instructions_config,
            "--skip-git-repo-check",
            "--dangerously-bypass-approvals-and-sandbox",
            "-m",
            model,
            "--output-last-message",
            &out,
            session_id,
            "-",
        ]);
    }

    strings([
        "exec",
        "--json",
        "--color",
        "never",
        "-c",
        &instructions_config,
        "--cd",
        &workdir,
        "--skip-git-repo-check",
        "--dangerously-bypass-approvals-and-sandbox",
        "-m",
        model,
        "--output-last-message",
        &out,
        "-",
    ])
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

pub fn codex_env(cfg: &CodexConfig) -> Vec<(String, String)> {
    vec![
        (
            "HOME".to_string(),
            cfg.user_home.to_string_lossy().to_string(),
        ),
        (
            "CODEX_HOME".to_string(),
            cfg.home.to_string_lossy().to_string(),
        ),
        (
            "XDG_CONFIG_HOME".to_string(),
            cfg.xdg_config_home.to_string_lossy().to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            cfg.xdg_cache_home.to_string_lossy().to_string(),
        ),
        (
            "XDG_DATA_HOME".to_string(),
            cfg.xdg_data_home.to_string_lossy().to_string(),
        ),
        (
            "XDG_STATE_HOME".to_string(),
            cfg.xdg_state_home.to_string_lossy().to_string(),
        ),
        ("PATH".to_string(), cfg.path.clone()),
        ("LANG".to_string(), "en_US.UTF-8".to_string()),
        ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
    ]
}

pub fn run_codex(
    cfg: &CodexConfig,
    prompt: &str,
    session_id: Option<&str>,
    model: &str,
    timeout: Duration,
    state_dir: &Path,
) -> Result<CodexOutput, String> {
    fs::create_dir_all(state_dir).map_err(|err| format!("create state dir: {err}"))?;
    write_main_agents_md(&cfg.instructions_file)?;
    let out_file = tempfile::NamedTempFile::new_in(state_dir).map_err(|err| err.to_string())?;
    let out_path = out_file.path().to_path_buf();
    let args = codex_args(
        &out_path,
        session_id,
        model,
        &cfg.default_model,
        &cfg.workdir,
        &cfg.instructions_file,
    );

    let mut child = Command::new(&cfg.bin)
        .args(args)
        .current_dir(&cfg.workdir)
        .env_clear()
        .envs(codex_env(cfg))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("start codex: {err}"))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| "open codex stdin".to_string())?
        .write_all(prompt.as_bytes())
        .map_err(|err| format!("write codex stdin: {err}"))?;
    drop(child.stdin.take());

    let start = Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            let final_text = final_text_from_outputs(&out_path, &output.stdout, &output.stderr);
            return Err(format!("codex timed out after {timeout:?}\n\n{final_text}"));
        }
        if child.try_wait().map_err(|err| err.to_string())?.is_some() {
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            let parsed = parse_codex_json(&String::from_utf8_lossy(&output.stdout));
            let final_text = fs::read_to_string(&out_path)
                .unwrap_or_default()
                .trim()
                .to_string();
            let final_text = if final_text.is_empty() {
                parsed.final_text
            } else {
                final_text
            };
            if output.status.success() {
                return Ok(CodexOutput {
                    final_text,
                    session_id: parsed.session_id,
                });
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err([final_text.as_str(), stderr.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn write_main_agents_md(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("create instructions dir: {err}"))?;
    }
    fs::write(path, include_str!("AGENTS.md")).map_err(|err| format!("write AGENTS.md: {err}"))
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

    #[test]
    fn codex_args_use_yolo_flag_for_new_session() {
        let args = codex_args(
            Path::new("/tmp/out"),
            None,
            "",
            "gpt-5.5",
            Path::new("/work"),
            Path::new("/state/AGENTS.md"),
        );
        let joined = args.join(" ");

        assert!(joined.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(joined.contains("--color never"));
        assert!(joined.contains("--cd /work"));
        assert!(joined.contains("-c model_instructions_file=\"/state/AGENTS.md\""));
        assert!(!joined.contains("--ask-for-approval"));
        assert!(!joined.contains("--sandbox"));
    }

    #[test]
    fn codex_args_resume_session() {
        let args = codex_args(
            Path::new("/tmp/out"),
            Some("session-123"),
            "gpt-test",
            "gpt-5.5",
            Path::new("/work"),
            Path::new("/state/AGENTS.md"),
        );
        assert_eq!(
            args.join(" "),
            "exec resume --json -c model_instructions_file=\"/state/AGENTS.md\" --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox -m gpt-test --output-last-message /tmp/out session-123 -"
        );
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
    fn codex_env_is_trimmed() {
        let cfg = CodexConfig {
            bin: PathBuf::from("/bin/codex"),
            home: PathBuf::from("/codex-home"),
            user_home: PathBuf::from("/home/example"),
            xdg_config_home: PathBuf::from("/xdg/config"),
            xdg_cache_home: PathBuf::from("/xdg/cache"),
            xdg_data_home: PathBuf::from("/xdg/data"),
            xdg_state_home: PathBuf::from("/xdg/state"),
            workdir: PathBuf::from("/work"),
            path: "/bin:/usr/bin".to_string(),
            default_model: "gpt-5.5".to_string(),
            instructions_file: PathBuf::from("/state/AGENTS.md"),
        };

        let env = codex_env(&cfg);

        assert!(env.contains(&("CODEX_HOME".to_string(), "/codex-home".to_string())));
        assert!(env.contains(&("HOME".to_string(), "/home/example".to_string())));
        assert!(env.contains(&("XDG_CONFIG_HOME".to_string(), "/xdg/config".to_string())));
        assert!(env.contains(&("XDG_CACHE_HOME".to_string(), "/xdg/cache".to_string())));
        assert!(env.contains(&("XDG_DATA_HOME".to_string(), "/xdg/data".to_string())));
        assert!(env.contains(&("XDG_STATE_HOME".to_string(), "/xdg/state".to_string())));
        assert!(env.contains(&("LANG".to_string(), "en_US.UTF-8".to_string())));
        assert!(env.contains(&("PATH".to_string(), "/bin:/usr/bin".to_string())));
    }
}
