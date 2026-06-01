# Pragmatic Quality Pass Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the current clippy blockers, add a tested one-second launchctl delay, preserve behavior, verify tests and coverage, and run a scoped mutation analysis using Homebrew-installed tooling when available.

**Architecture:** This is a local refactor plus a targeted setup-script sequencing fix. `src/bot.rs` gets a small context struct for model-selection routing data, `src/codex.rs` gets a `CodexRun` request struct to group run-specific Codex options, and `setup` sleeps between launchctl transitions.

**Tech Stack:** Rust 2021, Cargo, Clippy, cargo-llvm-cov, Homebrew, cargo-mutants.

---

## File Structure

1. Modify `src/bot.rs`.
   - Responsibility: Telegram bot command handling and job orchestration.
   - Change: add `ModelSelectionContext<'a>` and pass it into `select_model_slot`.
2. Modify `src/codex.rs`.
   - Responsibility: Codex CLI argument construction and process execution.
   - Change: add `CodexRun<'a>` and pass it into `run_codex_stream`.
3. Optionally modify tests in `src/bot.rs` or `src/codex.rs`.
   - Responsibility: preserve behavior around model selection and Codex stream invocation.
   - Change only if the compile/refactor exposes an uncovered high-value assertion.
4. Modify `setup`.
   - Responsibility: build and register the LaunchAgent.
   - Change: add `sleep 1` after `launchctl bootout` and after `launchctl bootstrap`.
5. Modify `src/launchd.rs`.
   - Responsibility: Rust tests for setup and launch scripts.
   - Change: stub `sleep` in the setup test and assert launchctl/sleep sequencing.
6. Modify `README.md`.
   - Responsibility: user-facing setup documentation.
   - Change: mention `sleep` in setup prerequisites and the launchctl delay.

---

### Task 1: Reduce Bot Model Selection Arguments

**Files:**
- Modify: `src/bot.rs`

- [ ] **Step 1: Inspect the current call and callee**

Run: `sed -n '390,470p' src/bot.rs`

Expected: output shows `handle_model_command` calling `select_model_slot` with `cfg`, `tg`, `store`, `selections`, `msg.chat.id`, `msg.message_id`, `key`, and `index`.

- [ ] **Step 2: Introduce the local context struct**

Add this struct near `select_model_slot`:

```rust
struct ModelSelectionContext<'a> {
    chat_id: i64,
    reply_to_message_id: i64,
    key: &'a SessionKey,
}
```

- [ ] **Step 3: Update the call site**

Replace the `select_model_slot` call in `handle_model_command` with:

```rust
    select_model_slot(
        cfg,
        tg,
        store,
        selections,
        ModelSelectionContext {
            chat_id: msg.chat.id,
            reply_to_message_id: msg.message_id,
            key,
        },
        index,
    )
```

- [ ] **Step 4: Update the callee signature and body**

Change `select_model_slot` to:

```rust
fn select_model_slot(
    cfg: &Config,
    tg: &impl TelegramApi,
    store: &SessionStore,
    selections: &RuntimeSelections,
    context: ModelSelectionContext<'_>,
    index: usize,
) -> Result<(), String> {
    let Some(choice) = cfg.provider_model_at(index).cloned() else {
        return tg.send_message(
            context.chat_id,
            &format!(
                "🧭 Unknown model slot {index}. Use /model and choose one of 0..{}.",
                cfg.models.len().saturating_sub(1)
            ),
            context.reply_to_message_id,
        );
    };
    set_selection(selections, context.key, choice.clone());
    store.reset(context.key)?;
    tg.send_message(
        context.chat_id,
        &format!(
            "🤖 Selected {}\n🧵 Session: none",
            provider_model_label(&choice)
        ),
        context.reply_to_message_id,
    )
}
```

- [ ] **Step 5: Verify the edited bot tests still pass**

Run: `cargo test bot::tests::`

Expected: all bot tests pass.

---

### Task 2: Reduce Codex Stream Arguments

**Files:**
- Modify: `src/codex.rs`
- Modify: `src/bot.rs`
- Modify: `src/run_mode.rs`

- [ ] **Step 1: Inspect current Codex stream callers**

Run: `rg -n "run_codex_stream\\(" src`

Expected: output shows the function, the wrapper in `src/codex.rs`, and callers in `src/bot.rs`, `src/codex.rs` tests, and `src/run_mode.rs`.

- [ ] **Step 2: Add a run request struct**

Add this public struct after `CodexOutput` in `src/codex.rs`:

```rust
pub struct CodexRun<'a> {
    pub prompt: &'a str,
    pub session_id: Option<&'a str>,
    pub provider: Provider,
    pub model: &'a str,
    pub timeout: Duration,
    pub state_dir: &'a Path,
}
```

- [ ] **Step 3: Update `run_codex` wrapper**

Replace the `run_codex_stream` call in `run_codex` with:

```rust
    run_codex_stream(
        cfg,
        CodexRun {
            prompt,
            session_id,
            provider,
            model,
            timeout,
            state_dir,
        },
        |_| {},
    )
```

- [ ] **Step 4: Update `run_codex_stream` signature and internals**

Change the signature to:

```rust
pub fn run_codex_stream(
    cfg: &CodexConfig,
    run: CodexRun<'_>,
    mut on_stdout: impl FnMut(&str),
) -> Result<CodexOutput, String> {
```

Inside the function, replace field uses:

```rust
fs::create_dir_all(run.state_dir).map_err(|err| format!("create state dir: {err}"))?;
let out_file = tempfile::NamedTempFile::new_in(run.state_dir).map_err(|err| err.to_string())?;
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
)?;
```

Also replace:

```rust
stdin.write_all(run.prompt.as_bytes())
```

and:

```rust
if start.elapsed() > run.timeout {
```

- [ ] **Step 5: Update production callers**

In `src/bot.rs`, import or qualify `CodexRun` and call:

```rust
        run_codex_stream(
            &codex_cfg,
            CodexRun {
                prompt: &job.prompt,
                session_id: state.session_id.as_deref(),
                provider: state.provider,
                model: &state.model,
                timeout: cfg.timeout,
                state_dir: &cfg.state_dir,
            },
            |chunk| {
                stream.push_str(chunk);
                if last_edit.elapsed() >= Duration::from_millis(1200) {
                    let _ = tg.edit_message_text(
                        job.chat_id,
                        thinking_id,
                        &stream_preview(&stream),
                    );
                    last_edit = Instant::now();
                }
            },
        )
```

In `src/run_mode.rs`, keep using `run_codex` unless it already calls `run_codex_stream` directly.

- [ ] **Step 6: Update test callers in `src/codex.rs`**

For each direct `run_codex_stream` test call, replace the argument list with:

```rust
        let output = run_codex_stream(
            &cfg,
            CodexRun {
                prompt: "hello",
                session_id: None,
                provider: Provider::Codex,
                model: "gpt-test",
                timeout: Duration::from_secs(5),
                state_dir: dir.path(),
            },
            |_| {},
        )
        .unwrap();
```

Use each test's existing prompt, session ID, provider, model, timeout, state directory, and callback values.

- [ ] **Step 7: Verify Codex-focused tests**

Run: `cargo test codex::tests::`

Expected: all Codex tests pass.

---

### Task 3: Add Launchctl Delay

**Files:**
- Modify: `setup`
- Modify: `src/launchd.rs`
- Modify: `README.md`

- [ ] **Step 1: Confirm the current launchctl sequence**

Run: `sed -n '84,100p' setup`

Expected: output shows consecutive `launchctl bootout`, `launchctl bootstrap`, and `launchctl kickstart` commands.

- [ ] **Step 2: Add `sleep` to setup prerequisites**

In `setup`, add `sleep` to `gateway_required_commands`:

```zsh
gateway_required_commands=(
  cargo
  codex
  date
  fastfetch
  id
  jq
  launchctl
  mkdir
  mv
  rm
  sleep
)
```

- [ ] **Step 3: Add a one-second delay between launchctl commands**

Replace the launchctl sequence at the end of `setup` with:

```zsh
launchctl bootout "$gateway_domain/$gateway_label" 2>/dev/null || true
sleep 1
launchctl bootstrap "$gateway_domain" "$gateway_plist_dest"
sleep 1
launchctl kickstart -k "$gateway_domain/$gateway_label"
```

- [ ] **Step 4: Stub `sleep` in the setup test**

In `src/launchd.rs`, after creating the fake `launchctl`, add:

```rust
        let sleep = stub_dir.join("sleep");
        fs::write(
            &sleep,
            "#!/bin/zsh\nprint -- \"sleep $*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        )
        .unwrap();
        fs::set_permissions(&sleep, fs::Permissions::from_mode(0o700)).unwrap();
```

- [ ] **Step 5: Assert the sequence includes sleeps**

Replace the existing bootstrap-only assertion with:

```rust
        let launchctl_log = fs::read_to_string(launchctl_log).unwrap();
        assert!(launchctl_log.contains("bootout"));
        assert!(launchctl_log.contains("sleep 1\nbootstrap"));
        assert!(launchctl_log.contains("sleep 1\nkickstart"));
```

- [ ] **Step 6: Update setup documentation**

In `README.md`, include `sleep` in the setup command list and describe the launchctl sequence as:

```markdown
`$HOME/Library/LaunchAgents`, then runs launchd `bootout`, waits one second,
`bootstrap`, waits one second, and `kickstart`.
```

- [ ] **Step 7: Verify the setup test**

Run: `cargo test launchd::tests::setup_writes_launch_path_without_runtime_environment`

Expected: the setup test passes without waiting on the real `/bin/sleep`.

---

### Task 4: Cleanup and Static Verification

**Files:**
- Modify only files touched by Tasks 1, 2, and 3 if cleanup is needed.

- [ ] **Step 1: Run formatting**

Run: `cargo fmt`

Expected: command exits successfully.

- [ ] **Step 2: Run tests**

Run: `cargo test`

Expected: all tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets --all-features -- -D warnings`

Expected: command exits successfully with no warnings.

- [ ] **Step 4: Run coverage summary**

Run: `cargo llvm-cov --all-targets --summary-only`

Expected: command exits successfully and prints a coverage table. Overall line coverage should remain near the baseline of 93.10%; investigate any large regression.

---

### Task 5: Mutation Tooling and Scoped Mutation Analysis

**Files:**
- No source file changes expected unless mutation output reveals a small, high-value test gap.

- [ ] **Step 1: Check Homebrew formula availability**

Run: `brew search cargo-mutants`

Expected: output includes `cargo-mutants` or no exact formula.

- [ ] **Step 2: Install mutation tooling with Homebrew when available**

If Step 1 finds the formula, run: `brew install cargo-mutants`

Expected: install completes successfully.

If Step 1 does not find the formula, run: `brew install cargo-mutants` once and record the failure for the final audit.

- [ ] **Step 3: Verify the command**

Run: `cargo mutants --version`

Expected: prints a cargo-mutants version. If the command is unavailable, stop mutation execution and report the tooling blocker.

- [ ] **Step 4: Run scoped mutation analysis**

Run: `cargo mutants --file src/bot.rs --file src/codex.rs --file src/launchd.rs --timeout 120`

Expected: command completes or produces a bounded list of missed/killed/time-out mutants for edited files.

- [ ] **Step 5: Address only small, clear test gaps**

If mutation output identifies a surviving mutant in edited code that can be killed with a focused unit test, add that test in the same file's existing `#[cfg(test)]` module and rerun:

```zsh
cargo test
cargo mutants --file src/bot.rs --file src/codex.rs --file src/launchd.rs --timeout 120
```

Expected: the specific survivor is killed and the full test suite remains green.

---

### Task 6: Final Audit Evidence

**Files:**
- No source file changes expected.

- [ ] **Step 1: Capture final git status**

Run: `git status --short`

Expected: only intentional files are modified.

- [ ] **Step 2: Capture final diff summary**

Run: `git diff --stat`

Expected: diff includes the implementation files, this plan file if not committed, and no unrelated files.

- [ ] **Step 3: Prepare final CRAAP analysis**

Use evidence from:

1. Local source files and tests.
2. `cargo test`.
3. `cargo clippy --all-targets --all-features -- -D warnings`.
4. `cargo llvm-cov --all-targets --summary-only`.
5. `cargo mutants` or Homebrew install/version output.

Final CRAAP points:

1. Currency: commands were run during this work on 2026-06-01.
2. Relevance: evidence comes from this repository and edited files.
3. Authority: Cargo, Clippy, cargo-llvm-cov, Homebrew, and cargo-mutants are the direct project/toolchain sources.
4. Accuracy: claims are tied to command results and local diffs.
5. Purpose: audit is scoped to behavior-preserving quality improvement, not feature change.
