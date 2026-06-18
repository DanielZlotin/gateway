# Core Context Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add runtime-editable production gateway core context files that are ensured by Gateway entrypoints and injected into every Gateway-spawned Codex process as developer instructions.

**Architecture:** Add a focused `src/context.rs` module that owns template metadata, runtime file creation/header refresh, core-context reads, size limiting, and heartbeat prompt path creation. Thread `$XDG_CONFIG_HOME` into `CodexConfig`, read core context immediately before every Codex spawn, and keep `HEARTBEAT.md` out of normal core context. Keep setup unchanged.

**Tech Stack:** Rust 2021, standard library filesystem APIs, existing `cargo test` test suite, existing Codex CLI `-c developer_instructions=...` integration.

---

## File Structure

Create:

1. `src/context.rs`  
   Owns runtime context file specs, ensure logic, developer-instruction assembly, size limit, and unit tests.

2. `src/prompts/AGENTS.md`  
   Core template skeleton for gateway operating rules.

3. `src/prompts/IDENTITY.md`  
   Core template skeleton for assistant identity.

4. `src/prompts/USER.md`  
   Core template skeleton for user preferences.

5. `src/prompts/TOOLS.md`  
   Core template skeleton for environment/tool facts.

6. `src/prompts/MEMORY.md`  
   Core template skeleton for durable memory.

7. `src/prompts/SYSTEM.md`  
   Move the existing built-in runtime instructions here from `prompts/SYSTEM.md`.

8. `src/prompts/HEARTBEAT.md`  
   Heartbeat-only template skeleton.

Modify:

1. `src/lib.rs`  
   Export `context`.

2. `src/codex.rs`  
   Add `xdg_config_home` to `CodexConfig`, read developer instructions from `context`, and pass the assembled text to `codex_args`.

3. `src/main.rs`  
   Ensure context files once for `gateway bot`, `gateway heartbeat`, `gateway run`, and any current CLI mode that can construct `CodexConfig` and launch Codex, such as `gateway status`.

4. `src/heartbeat.rs`  
   Replace local heartbeat prompt creation with `context::ensure_heartbeat_prompt_file`.

5. `src/bot.rs`, `src/run_mode.rs`, `src/status.rs`, `src/cli_commands.rs` tests  
   Update test `CodexConfig` construction and `Config` fixtures for the added field where needed.

6. `README.md`  
   Document `$XDG_CONFIG_HOME/gateway/*.md` context files and heartbeat separation.

Delete:

1. `prompts/SYSTEM.md`
2. `prompts/HEARTBEAT.md`

The old root-level prompt directory should not remain as a second template source.

---

### Task 1: Add Context Module Tests And Templates

**Files:**
- Create: `src/context.rs`
- Create: `src/prompts/AGENTS.md`
- Create: `src/prompts/IDENTITY.md`
- Create: `src/prompts/USER.md`
- Create: `src/prompts/TOOLS.md`
- Create: `src/prompts/MEMORY.md`
- Create: `src/prompts/HEARTBEAT.md`
- Create: `src/prompts/SYSTEM.md`
- Modify: `src/lib.rs`
- Delete after includes are migrated in Task 2: `prompts/SYSTEM.md`
- Delete after includes are migrated in Task 3: `prompts/HEARTBEAT.md`

- [ ] **Step 1: Add the failing context module tests**

Create `src/context.rs` with only constants, function signatures, and the tests below. The functions should return `unimplemented!()` so the tests compile and fail on behavior.

```rust
use std::fs;
use std::path::{Path, PathBuf};

pub const MAX_DEVELOPER_INSTRUCTIONS_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
struct ContextFile {
    filename: &'static str,
    template: &'static str,
}

const SYSTEM_TEMPLATE: &str = include_str!("prompts/SYSTEM.md");
const AGENTS_TEMPLATE: &str = include_str!("prompts/AGENTS.md");
const IDENTITY_TEMPLATE: &str = include_str!("prompts/IDENTITY.md");
const USER_TEMPLATE: &str = include_str!("prompts/USER.md");
const TOOLS_TEMPLATE: &str = include_str!("prompts/TOOLS.md");
const MEMORY_TEMPLATE: &str = include_str!("prompts/MEMORY.md");
const HEARTBEAT_TEMPLATE: &str = include_str!("prompts/HEARTBEAT.md");

const CORE_CONTEXT_FILES: &[ContextFile] = &[
    ContextFile {
        filename: "AGENTS.md",
        template: AGENTS_TEMPLATE,
    },
    ContextFile {
        filename: "IDENTITY.md",
        template: IDENTITY_TEMPLATE,
    },
    ContextFile {
        filename: "USER.md",
        template: USER_TEMPLATE,
    },
    ContextFile {
        filename: "TOOLS.md",
        template: TOOLS_TEMPLATE,
    },
    ContextFile {
        filename: "MEMORY.md",
        template: MEMORY_TEMPLATE,
    },
];

const HEARTBEAT_CONTEXT_FILE: ContextFile = ContextFile {
    filename: "HEARTBEAT.md",
    template: HEARTBEAT_TEMPLATE,
};

pub fn ensure_gateway_context_files(_xdg_config_home: &Path) -> Result<(), String> {
    unimplemented!("implemented in Task 1 green step")
}

pub fn ensure_heartbeat_prompt_file(_xdg_config_home: &Path) -> Result<PathBuf, String> {
    unimplemented!("implemented in Task 1 green step")
}

pub fn developer_instructions(_xdg_config_home: &Path) -> Result<String, String> {
    unimplemented!("implemented in Task 1 green step")
}

fn gateway_context_dir(xdg_config_home: &Path) -> PathBuf {
    xdg_config_home.join("gateway")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_gateway_context_files_creates_core_and_heartbeat_files() {
        let dir = tempfile::tempdir().unwrap();

        ensure_gateway_context_files(&dir.path().join("config")).unwrap();

        for filename in [
            "AGENTS.md",
            "IDENTITY.md",
            "USER.md",
            "TOOLS.md",
            "MEMORY.md",
            "HEARTBEAT.md",
        ] {
            let path = dir.path().join("config/gateway").join(filename);
            let text = fs::read_to_string(&path).unwrap();
            assert!(text.starts_with("# "), "{filename} should start with a title");
            assert!(
                text.lines().nth(1).unwrap_or_default().starts_with("> **Scope:**"),
                "{filename} should include a scope line: {text}"
            );
        }
    }

    #[test]
    fn ensure_gateway_context_files_refreshes_header_and_preserves_body() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let gateway_dir = config_home.join("gateway");
        fs::create_dir_all(&gateway_dir).unwrap();
        fs::write(
            gateway_dir.join("USER.md"),
            "# Old title\n> **Scope:** old scope\n\n- Keep this user preference.\n",
        )
        .unwrap();

        ensure_gateway_context_files(&config_home).unwrap();

        let text = fs::read_to_string(gateway_dir.join("USER.md")).unwrap();
        assert!(text.starts_with("# USER.md\n> **Scope:** user identity"));
        assert!(text.contains("- Keep this user preference."));
    }

    #[test]
    fn developer_instructions_include_core_files_in_order_and_exclude_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        ensure_gateway_context_files(&config_home).unwrap();

        let text = developer_instructions(&config_home).unwrap();

        assert!(text.starts_with("# 🌉 Gateway Runtime Instructions"));
        let agents = text.find("# AGENTS.md").unwrap();
        let identity = text.find("# IDENTITY.md").unwrap();
        let user = text.find("# USER.md").unwrap();
        let tools = text.find("# TOOLS.md").unwrap();
        let memory = text.find("# MEMORY.md").unwrap();
        assert!(agents < identity);
        assert!(identity < user);
        assert!(user < tools);
        assert!(tools < memory);
        assert!(!text.contains("# HEARTBEAT.md"));
    }

    #[test]
    fn developer_instructions_fail_when_core_context_is_too_large() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        ensure_gateway_context_files(&config_home).unwrap();
        fs::write(
            config_home.join("gateway/MEMORY.md"),
            format!(
                "# MEMORY.md\n> **Scope:** durable learned facts and standing instructions.\n\n{}",
                "x".repeat(MAX_DEVELOPER_INSTRUCTIONS_BYTES)
            ),
        )
        .unwrap();

        let err = developer_instructions(&config_home).unwrap_err();

        assert!(err.contains("gateway core context is too large"), "{err}");
        assert!(err.contains("MEMORY.md"), "{err}");
    }

    #[test]
    fn ensure_heartbeat_prompt_file_returns_heartbeat_path() {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");

        let path = ensure_heartbeat_prompt_file(&config_home).unwrap();

        assert_eq!(path, config_home.join("gateway/HEARTBEAT.md"));
        let text = fs::read_to_string(path).unwrap();
        assert!(text.starts_with("# HEARTBEAT.md\n> **Scope:** scheduled heartbeat protocol"));
    }
}
```

Add `pub mod context;` to `src/lib.rs`.

- [ ] **Step 2: Add the template files required by the tests**

Create these files exactly:

`src/prompts/AGENTS.md`

```markdown
# AGENTS.md
> **Scope:** gateway operating rules, context-loading policy, safety boundaries, instruction precedence, and ownership rules for the other core files.
```

`src/prompts/IDENTITY.md`

```markdown
# IDENTITY.md
> **Scope:** assistant identity only.
```

`src/prompts/USER.md`

```markdown
# USER.md
> **Scope:** user identity, preferences, language, communication style, and shorthands.
```

`src/prompts/TOOLS.md`

```markdown
# TOOLS.md
> **Scope:** local environment facts, tool availability, command conventions, service endpoints, and operational recipes.
```

`src/prompts/MEMORY.md`

```markdown
# MEMORY.md
> **Scope:** durable learned facts and standing instructions that do not belong in a more specific file.
```

`src/prompts/HEARTBEAT.md`

```markdown
# HEARTBEAT.md
> **Scope:** scheduled heartbeat protocol only.
```

Copy the current `prompts/SYSTEM.md` content into `src/prompts/SYSTEM.md` unchanged.

- [ ] **Step 3: Run the new tests to verify they fail for missing behavior**

Run:

```zsh
cargo test context::tests --lib
```

Expected: the tests compile and fail because `ensure_gateway_context_files`, `ensure_heartbeat_prompt_file`, and `developer_instructions` are not implemented.

- [ ] **Step 4: Implement the minimal context module**

Replace the three `unimplemented!()` functions and add helpers in `src/context.rs`:

```rust
pub fn ensure_gateway_context_files(xdg_config_home: &Path) -> Result<(), String> {
    for spec in CORE_CONTEXT_FILES {
        ensure_context_file(xdg_config_home, *spec)?;
    }
    ensure_context_file(xdg_config_home, HEARTBEAT_CONTEXT_FILE)?;
    Ok(())
}

pub fn ensure_heartbeat_prompt_file(xdg_config_home: &Path) -> Result<PathBuf, String> {
    ensure_context_file(xdg_config_home, HEARTBEAT_CONTEXT_FILE)
}

pub fn developer_instructions(xdg_config_home: &Path) -> Result<String, String> {
    let mut parts = vec![SYSTEM_TEMPLATE.trim_end().to_string()];
    let mut largest_file = ("SYSTEM.md", SYSTEM_TEMPLATE.len());
    for spec in CORE_CONTEXT_FILES {
        let path = context_file_path(xdg_config_home, spec.filename);
        let text = fs::read_to_string(&path)
            .map_err(|err| format!("read gateway context file {}: {err}", path.display()))?;
        if text.len() > largest_file.1 {
            largest_file = (spec.filename, text.len());
        }
        parts.push(text.trim_end().to_string());
    }
    let text = parts.join("\n\n");
    if text.len() > MAX_DEVELOPER_INSTRUCTIONS_BYTES {
        return Err(format!(
            "gateway core context is too large ({} bytes, limit {} bytes); trim $XDG_CONFIG_HOME/gateway/{}",
            text.len(),
            MAX_DEVELOPER_INSTRUCTIONS_BYTES,
            largest_file.0
        ));
    }
    Ok(text)
}

fn ensure_context_file(
    xdg_config_home: &Path,
    spec: ContextFile,
) -> Result<PathBuf, String> {
    let path = context_file_path(xdg_config_home, spec.filename);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create gateway context dir {}: {err}", parent.display()))?;
    }
    let new_text = match fs::read_to_string(&path) {
        Ok(existing) => refresh_header(spec.template, &existing)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => template_header(spec.template)?,
        Err(err) => {
            return Err(format!(
                "read gateway context file {}: {err}",
                path.display()
            ));
        }
    };
    fs::write(&path, new_text)
        .map_err(|err| format!("write gateway context file {}: {err}", path.display()))?;
    Ok(path)
}

fn context_file_path(xdg_config_home: &Path, filename: &str) -> PathBuf {
    gateway_context_dir(xdg_config_home).join(filename)
}

fn refresh_header(template: &str, existing: &str) -> Result<String, String> {
    let header = template_header(template)?;
    Ok(format!("{header}{}", content_after_first_two_lines(existing)))
}

fn template_header(template: &str) -> Result<String, String> {
    let mut lines = template.lines();
    let title = lines
        .next()
        .filter(|line| line.starts_with("# "))
        .ok_or_else(|| "gateway context template missing title line".to_string())?;
    let scope = lines
        .next()
        .filter(|line| line.starts_with("> **Scope:**"))
        .ok_or_else(|| "gateway context template missing scope line".to_string())?;
    Ok(format!("{title}\n{scope}\n"))
}

fn content_after_first_two_lines(text: &str) -> &str {
    let mut start = 0;
    for _ in 0..2 {
        let Some(offset) = text[start..].find('\n') else {
            return "";
        };
        start += offset + 1;
    }
    &text[start..]
}
```

- [ ] **Step 5: Run the context tests to verify green**

Run:

```zsh
cargo test context::tests --lib
```

Expected: all `context::tests` pass.

- [ ] **Step 6: Commit Task 1**

Run:

```zsh
git add src/context.rs src/lib.rs src/prompts/AGENTS.md src/prompts/IDENTITY.md src/prompts/USER.md src/prompts/TOOLS.md src/prompts/MEMORY.md src/prompts/HEARTBEAT.md src/prompts/SYSTEM.md
git commit -m "✨ feat: add gateway context file manager"
```

---

### Task 2: Inject Core Context Into Codex Arguments

**Files:**
- Modify: `src/codex.rs`
- Modify test helpers in: `src/bot.rs`, `src/run_mode.rs`, `src/status.rs`

- [ ] **Step 1: Write failing Codex tests**

In `src/codex.rs`, add these tests inside `mod tests`:

```rust
#[test]
fn codex_args_use_runtime_core_context_as_developer_instructions() {
    let developer_instructions = "# system\n\n# AGENTS.md\nagent rules";
    let args = codex_args(
        Path::new("/tmp/out"),
        None,
        crate::provider::Provider::Codex,
        "",
        "gpt-5.5",
        Path::new("/work"),
        None,
        &[],
        developer_instructions,
    )
    .unwrap();
    let joined = args.join(" ");

    assert!(joined.contains("developer_instructions=\"# system\\n\\n# AGENTS.md\\nagent rules\""));
    assert!(!joined.contains("model_instructions_file"));
}

#[test]
fn run_codex_stream_reads_context_before_starting_codex() {
    let dir = tempdir().unwrap();
    let fake_codex = executable(
        dir.path().join("codex-context"),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "{}"
out=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then out="$arg"; fi
  prev="$arg"
done
cat >/dev/null
printf 'OK\n' > "$out"
"#,
            dir.path().join("codex.args").display()
        ),
    );
    let cfg = codex_config(&fake_codex, dir.path());
    crate::context::ensure_gateway_context_files(&cfg.xdg_config_home).unwrap();
    fs::write(
        cfg.xdg_config_home.join("gateway/MEMORY.md"),
        "# MEMORY.md\n> **Scope:** durable learned facts and standing instructions that do not belong in a more specific file.\n\n- runtime memory marker\n",
    )
    .unwrap();

    run_codex_stream(
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

    let args = fs::read_to_string(dir.path().join("codex.args")).unwrap();
    assert!(args.contains("# AGENTS.md"), "{args}");
    assert!(args.contains("runtime memory marker"), "{args}");
    assert!(!args.contains("# HEARTBEAT.md"), "{args}");
}
```

Update existing direct `codex_args(...)` test calls to pass `GATEWAY_DEVELOPER_INSTRUCTIONS` as the final argument so they compile.

- [ ] **Step 2: Run the Codex tests to verify red**

Run:

```zsh
cargo test codex::tests --lib
```

Expected: compile failure or test failure because `codex_args` does not accept a developer instructions argument and `CodexConfig` does not contain `xdg_config_home`.

- [ ] **Step 3: Update `CodexConfig` and `codex_args`**

Modify `src/codex.rs`:

```rust
use crate::context;
```

Add the field:

```rust
#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub bin: PathBuf,
    pub workdir: PathBuf,
    pub xdg_config_home: PathBuf,
    pub default_model: String,
}
```

Update `From<&Config>`:

```rust
impl From<&Config> for CodexConfig {
    fn from(cfg: &Config) -> Self {
        Self {
            bin: PathBuf::from("codex"),
            workdir: cfg.codex_workdir.clone(),
            xdg_config_home: cfg.xdg_config_home.clone(),
            default_model: cfg.default_provider_model().model.clone(),
        }
    }
}
```

Change `codex_args` signature:

```rust
pub fn codex_args(
    out_path: &Path,
    session_id: Option<&str>,
    provider: Provider,
    model: &str,
    default_model: &str,
    workdir: &Path,
    claude_proxy_base_url: Option<&str>,
    image_paths: &[PathBuf],
    developer_instructions: &str,
) -> Result<Vec<String>, String> {
```

Replace the existing developer instruction config construction with:

```rust
let developer_instructions_config = format!(
    "developer_instructions={}",
    serde_json::to_string(developer_instructions)
        .expect("gateway developer instructions should serialize")
);
```

In `run_codex_stream`, before `codex_args(...)`, add:

```rust
let developer_instructions = context::developer_instructions(&cfg.xdg_config_home)?;
```

Pass `&developer_instructions` as the final `codex_args` argument.

- [ ] **Step 4: Update `CodexConfig` test fixtures**

Every explicit `CodexConfig { ... }` literal in tests must include:

```rust
xdg_config_home: root.join("config"),
```

For helper functions that receive `root: &Path`, use:

```rust
xdg_config_home: root.join("config"),
```

For fixtures that currently use a fixed path, use:

```rust
xdg_config_home: PathBuf::from("/xdg/config"),
```

- [ ] **Step 5: Run Codex tests to verify green**

Run:

```zsh
cargo test codex::tests --lib
```

Expected: all `codex::tests` pass.

- [ ] **Step 6: Commit Task 2**

Run:

```zsh
git add src/codex.rs src/bot.rs src/run_mode.rs src/status.rs
git commit -m "✨ feat: inject gateway context into codex runs"
```

---

### Task 3: Ensure Context Files At Gateway Entrypoints And Heartbeat

**Files:**
- Modify: `src/main.rs`
- Modify: `src/heartbeat.rs`
- Modify tests in: `src/heartbeat.rs`

- [ ] **Step 1: Write failing entrypoint and heartbeat tests**

In `src/main.rs`, add unit-testable helpers by extracting context ensure into a function:

```rust
fn ensure_context_for_mode(mode: &Mode, cfg: &gateway::config::Config) -> Result<(), String> {
    match mode {
        Mode::Bot | Mode::Heartbeat | Mode::Run(_) | Mode::Status(_) => {
            gateway::context::ensure_gateway_context_files(&cfg.xdg_config_home)
        }
        Mode::List(_) | Mode::Logs(_) | Mode::Update | Mode::Uninstall | Mode::Version => Ok(()),
    }
}
```

Add tests in `src/main.rs` under `#[cfg(test)]`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gateway::cli::{ChatArgs, RunArgs};
    use gateway::config::{Config, ModelRole, ProviderModel, TelegramBotConfig};
    use gateway::provider::Provider;
    use std::path::Path;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn ensure_context_for_codex_modes_creates_context_files() {
        for mode in [
            Mode::Bot,
            Mode::Heartbeat,
            Mode::Run(RunArgs {
                prompt: Some("x".to_string()),
                prompt_file: None,
                model: None,
                chat: None,
            }),
            Mode::Status(ChatArgs { chat: None }),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let cfg = test_config(dir.path());

            ensure_context_for_mode(&mode, &cfg).unwrap();

            assert!(cfg.xdg_config_home.join("gateway/AGENTS.md").exists());
            assert!(cfg.xdg_config_home.join("gateway/MEMORY.md").exists());
            assert!(cfg.xdg_config_home.join("gateway/HEARTBEAT.md").exists());
        }
    }

    #[test]
    fn ensure_context_for_non_codex_modes_does_not_create_context_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = test_config(dir.path());

        ensure_context_for_mode(&Mode::Logs(10), &cfg).unwrap();

        assert!(!cfg.xdg_config_home.join("gateway/AGENTS.md").exists());
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
                model: "gpt-default".to_string(),
                role: ModelRole::Default,
            }],
            tts: None,
            state_dir: state_dir.clone(),
            chat_state_dir: state_dir.join("chats"),
            offset_file: state_dir.join("telegram.offset"),
            gateway_log_file: state_dir.join("logs/gateway.log"),
            launchd_target: "gui/<uid>/ai.gateway".to_string(),
            poll_timeout_sec: 50,
            queue_depth: 8,
            codex_timeout: Duration::from_secs(30),
            heartbeat_interval: Duration::from_secs(60),
        }
    }
}
```

In `src/heartbeat.rs`, update the existing heartbeat prompt tests to expect a scope-only heartbeat template:

```rust
assert!(prompt.starts_with("# HEARTBEAT.md\n> **Scope:** scheduled heartbeat protocol only."));
```

- [ ] **Step 2: Run entrypoint and heartbeat tests to verify red**

Run:

```zsh
cargo test --bin gateway ensure_context_for_codex_modes_creates_context_files
cargo test heartbeat::tests::heartbeat_creates_custom_prompt_file_from_skeleton_when_missing --lib
```

Expected: the binary test fails until helper wiring exists; the heartbeat test fails until heartbeat uses `context::ensure_heartbeat_prompt_file` and the new template.

- [ ] **Step 3: Wire context ensure in `main.rs`**

Modify `run()` in `src/main.rs` so config-loading modes call the helper:

```rust
fn run() -> Result<(), String> {
    let mode = match parse_cli_from(std::env::args_os())? {
        CliAction::Execute(mode) => mode,
        CliAction::Help(help) => {
            print!("{help}");
            return Ok(());
        }
    };
    match mode {
        Mode::Bot => {
            let cfg = gateway::config::load()?;
            ensure_context_for_mode(&Mode::Bot, &cfg)?;
            gateway::bot::run(cfg)
        }
        Mode::Heartbeat => {
            let cfg = gateway::config::load()?;
            ensure_context_for_mode(&Mode::Heartbeat, &cfg)?;
            print_output(gateway::heartbeat::run(cfg))
        }
        Mode::List(args) => {
            print_output(gateway::cli_commands::list(args, gateway::config::load()?))
        }
        Mode::Logs(lines) => print_output(gateway::logs::read_gateway_logs(
            &gateway::config::current_env(),
            lines,
        )),
        Mode::Run(args) => {
            let cfg = gateway::config::load()?;
            ensure_context_for_mode(&Mode::Run(args.clone()), &cfg)?;
            print_output(gateway::run_mode::run(args, cfg))
        }
        Mode::Status(args) => {
            let cfg = gateway::config::load()?;
            ensure_context_for_mode(&Mode::Status(args.clone()), &cfg)?;
            print_output(gateway::cli_commands::status(args, cfg))
        }
        Mode::Update => print_output(gateway::cli_commands::update(gateway::config::load()?)),
        Mode::Uninstall => print_output(gateway::launchd::uninstall()),
        Mode::Version => print_output(Ok(format!("gateway {}", env!("CARGO_PKG_VERSION")))),
    }
}
```

If `RunArgs` and `ChatArgs` do not implement `Clone`, add `Clone` to their derives in `src/cli.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatArgs {
    pub chat: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub prompt: Option<String>,
    pub prompt_file: Option<PathBuf>,
    pub model: Option<String>,
    pub chat: Option<i64>,
}
```

Keep `ensure_context_for_mode` exactly as shown in Step 1.

- [ ] **Step 4: Replace heartbeat-specific file creation**

In `src/heartbeat.rs`, remove `HEARTBEAT_PROMPT_SKELETON`, remove the local `ensure_heartbeat_prompt_file` function, and replace:

```rust
let heartbeat_file = ensure_heartbeat_prompt_file(&cfg)?;
```

with:

```rust
let heartbeat_file = crate::context::ensure_heartbeat_prompt_file(&cfg.xdg_config_home)?;
```

- [ ] **Step 5: Run the entrypoint and heartbeat tests to verify green**

Run:

```zsh
cargo test --bin gateway ensure_context_for_codex_modes_creates_context_files
cargo test heartbeat::tests --lib
```

Expected: requested tests pass.

- [ ] **Step 6: Commit Task 3**

Run:

```zsh
git add src/main.rs src/cli.rs src/heartbeat.rs
git commit -m "🔧 wire gateway context setup into entrypoints"
```

---

### Task 4: Move Existing Prompt Includes And Delete Old Prompt Directory

**Files:**
- Modify: `src/codex.rs`
- Modify: `src/context.rs`
- Delete: `prompts/SYSTEM.md`
- Delete: `prompts/HEARTBEAT.md`

- [ ] **Step 1: Write failing assertion for the new include path**

In `src/context.rs`, add:

```rust
#[test]
fn system_template_is_loaded_from_context_module() {
    assert!(SYSTEM_TEMPLATE.contains("Gateway Runtime Instructions"));
}
```

This test should already pass once `src/prompts/SYSTEM.md` exists. Its purpose is to lock the new source of truth.

- [ ] **Step 2: Update includes and remove old prompt files**

In `src/codex.rs`, remove:

```rust
const GATEWAY_DEVELOPER_INSTRUCTIONS: &str = include_str!("../prompts/SYSTEM.md");
```

If any tests still reference `GATEWAY_DEVELOPER_INSTRUCTIONS`, replace those references with:

```rust
crate::context::developer_instructions(&PathBuf::from("/xdg/config")).unwrap_or_else(|_| {
    "# 🌉 Gateway Runtime Instructions".to_string()
})
```

Prefer direct literal developer instruction strings in `codex_args` tests to avoid filesystem setup.

Delete the old files:

```zsh
git rm prompts/SYSTEM.md prompts/HEARTBEAT.md
```

- [ ] **Step 3: Run prompt include tests**

Run:

```zsh
cargo test context::tests::system_template_is_loaded_from_context_module --lib
cargo test codex::tests::developer_instructions_block_private_data_in_telegram --lib
```

Expected: tests pass, or the second test is renamed to test `context::developer_instructions` contains Telegram/privacy language.

- [ ] **Step 4: Commit Task 4**

Run:

```zsh
git add src/context.rs src/codex.rs prompts/SYSTEM.md prompts/HEARTBEAT.md
git commit -m "🧹 move gateway prompt templates under src"
```

---

### Task 5: Document Runtime Context Files

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a failing documentation check**

If this repo has no doc tests, use an in-memory check:

```zsh
rg -n '\\$XDG_CONFIG_HOME/gateway/(AGENTS|IDENTITY|USER|TOOLS|MEMORY|HEARTBEAT)\\.md|developer_instructions|HEARTBEAT.md' README.md
```

Expected before the README edit: missing some or all required entries.

- [ ] **Step 2: Add README documentation**

Add a short section after the existing config/path documentation:

```markdown
## 🧠 Gateway Context

Gateway maintains runtime-editable Markdown context files under
`$XDG_CONFIG_HOME/gateway/`.

Always loaded into Gateway-spawned Codex developer instructions:

1. ⚙️ `AGENTS.md` — gateway operating rules.
2. 🪪 `IDENTITY.md` — assistant identity.
3. 👤 `USER.md` — user preferences and shorthands.
4. 🧰 `TOOLS.md` — local environment and tool facts.
5. 🧠 `MEMORY.md` — durable facts that do not belong elsewhere.

Heartbeat-only:

1. 🫀 `HEARTBEAT.md` — used only as the `gateway heartbeat` prompt file.

Gateway creates missing files and refreshes only their title/scope lines at
`gateway run`, `gateway bot`, and `gateway heartbeat` startup. User content below
those lines is preserved. Manual Codex sessions outside Gateway do not use these
files automatically.
```

- [ ] **Step 3: Verify README mentions all files**

Run:

```zsh
for name in AGENTS IDENTITY USER TOOLS MEMORY HEARTBEAT; do rg -n "$name.md" README.md; done
```

Expected: each command prints at least one matching README line.

- [ ] **Step 4: Commit Task 5**

Run:

```zsh
git add README.md
git commit -m "📝 docs: document gateway context files"
```

---

### Task 6: Full Verification And Cleanup

**Files:**
- Review all modified files from Tasks 1-5.

- [ ] **Step 1: Run formatting**

Run:

```zsh
cargo fmt
```

Expected: command exits 0.

- [ ] **Step 2: Run full tests**

Run:

```zsh
cargo test
```

Expected: all tests pass.

- [ ] **Step 3: Run release build**

Run:

```zsh
cargo build --release
```

Expected: build exits 0.

- [ ] **Step 4: Inspect final diff for scope**

Run:

```zsh
git diff --stat HEAD
git diff -- src/context.rs src/codex.rs src/heartbeat.rs src/main.rs src/cli.rs src/lib.rs README.md
```

Expected: diff only reflects context-file support, prompt-template relocation, tests, and docs. No setup changes.

- [ ] **Step 5: Commit formatting-only changes if any**

Run only if `cargo fmt` or cleanup changed files after the previous commits:

```zsh
git add src/context.rs src/codex.rs src/heartbeat.rs src/main.rs src/cli.rs src/lib.rs README.md Cargo.toml Cargo.lock
git commit -m "🧹 tidy gateway context implementation"
```

Expected: commit succeeds if there were changes; skip this step if `git diff --quiet` reports no unstaged changes.

- [ ] **Step 6: Final status**

Run:

```zsh
git status --short
```

Expected: no uncommitted changes from this implementation remain. Pre-existing unrelated user changes may remain if they were present before execution; do not revert them.

---

## Self-Review

Spec coverage:

1. Core files and heartbeat-only split are implemented by Tasks 1 and 3.
2. Read order and developer-instruction injection are implemented by Tasks 1 and 2.
3. Template location under `src/prompts/` is implemented by Tasks 1 and 4.
4. Ensure-on-entrypoint and read-on-spawn behavior are implemented by Tasks 2 and 3.
5. No setup hook, no cache, no prompt prepending, and no fallback filenames are preserved by design.
6. Size-limit fail-closed behavior is implemented by Task 1 and exercised again through Task 2.

Placeholder scan:

1. The plan contains no placeholder markers and no unspecified implementation steps.
2. Each code-changing step includes the exact file and code shape to apply.

Type consistency:

1. `CodexConfig.xdg_config_home` is introduced in Task 2 and used consistently by `run_codex_stream`.
2. `context::ensure_gateway_context_files`, `context::ensure_heartbeat_prompt_file`, and `context::developer_instructions` are introduced once and reused consistently.
3. `RunArgs` and `ChatArgs` are made `Clone` only if `main.rs` needs ownership-preserving mode checks.
