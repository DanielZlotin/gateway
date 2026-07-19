# `/status` Diff Summary Implementation Plan

> **For agentic workers:** Use superpowers:dispatching-parallel-agents only for two or more tasks that can run concurrently without shared files, shared state, or sequential dependencies. Otherwise use superpowers:executing-plans and implement inline. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make dirty repositories in `/status` show complete, icon-led semantic diff summaries without count fallbacks or truncation.

**Architecture:** Keep the existing `src/status.rs` Git collection flow. Remove both character limits, normalize successful model output with an icon, and make the model call retry once before returning a concise error; clean repositories bypass the model.

**Tech Stack:** Rust standard library, existing Codex runner, existing unit and temporary-repository integration tests.

---

### Task 1: Preserve complete diff input and icon-led model output

**Files:**
- Modify: `src/status.rs`
- Test: `src/status.rs`

- [x] **Step 1: Write failing input/output tests**

Replace the truncation test and extend the summary formatting test:

```rust
#[test]
fn git_summary_input_keeps_the_complete_diff() {
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(repo.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("large.txt"), "before\n").unwrap();
    assert!(Command::new("git")
        .args(["-C"])
        .arg(repo.path())
        .args(["add", "large.txt"])
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("large.txt"), "a".repeat(12_001)).unwrap();

    let got = git_summary_input(repo.path(), &["AM large.txt".to_string()]).unwrap();

    assert!(got.contains(&"a".repeat(12_001)));
    assert!(!got.contains("[diff truncated]"));
}

#[test]
fn git_summary_input_includes_untracked_file_contents() {
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(repo.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("note.txt"), "semantic change\n").unwrap();

    let got = git_summary_input(repo.path(), &["?? note.txt".to_string()]).unwrap();

    assert!(got.contains("untracked file note.txt:\nsemantic change"));
}

#[test]
fn concise_git_summary_keeps_complete_output_and_ensures_an_icon() {
    let summary = "x".repeat(181);

    assert_eq!(
        concise_git_summary(&summary),
        Some(format!("📝 {summary}"))
    );
    assert_eq!(
        concise_git_summary("✨ Full semantic summary"),
        Some("✨ Full semantic summary".to_string())
    );
    assert_eq!(concise_git_summary(" \n\t"), None);
}
```

- [x] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test status::tests::git_summary_input_
cargo test status::tests::concise_git_summary_keeps_complete_output_and_ensures_an_icon
```

Expected: FAIL because input is limited to 12,000 characters and output is limited to 180 characters without guaranteed icons.

- [x] **Step 3: Implement complete input and icon normalization**

Remove `MAX_GIT_SUMMARY_INPUT_CHARS` and `limit_git_summary_input`. Return the complete redacted status, stat, and patch join:

```rust
Ok(redact_private_data(&sections.join("\n\n")))
```

Add untracked file contents so untracked-only repositories can also receive a semantic summary:

```rust
fn push_untracked_file_sections(path: &Path, sections: &mut Vec<String>) -> Result<(), String> {
    let files = run_git_text(path, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for relative in files.split('\0').filter(|relative| !relative.is_empty()) {
        let file = path.join(relative);
        let metadata = fs::symlink_metadata(&file).map_err(|err| err.to_string())?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(file).map_err(|err| err.to_string())?;
            sections.push(format!(
                "untracked symlink {relative} -> {}",
                target.display()
            ));
            continue;
        }
        if !metadata.is_file() {
            sections.push(format!("untracked special file {relative}"));
            continue;
        }
        let content = fs::read(file).map_err(|err| err.to_string())?;
        sections.push(format!(
            "untracked file {relative}:\n{}",
            String::from_utf8_lossy(&content)
        ));
    }
    Ok(())
}
```

Call this helper after staged and unstaged patches and before the single redaction
pass. Test that symlinks are described without reading their targets.

Update summary normalization:

```rust
fn concise_git_summary(text: &str) -> Option<String> {
    let summary = text
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '`' | '*'))
        .trim();
    if summary.is_empty() {
        None
    } else if summary
        .chars()
        .next()
        .is_some_and(|ch| matches!(ch as u32, 0x1F000..=0x1FAFF | 0x2600..=0x27BF))
    {
        Some(summary.to_string())
    } else {
        Some(format!("📝 {summary}"))
    }
}
```

Change `git_summary_prompt` to require one relevant leading icon and remove wording that permits metadata-only summaries.

- [x] **Step 4: Run the focused tests and verify GREEN**

Run:

```bash
cargo test status::tests::git_summary_input_
cargo test status::tests::concise_git_summary_keeps_complete_output_and_ensures_an_icon
```

Expected: both tests PASS.

### Task 2: Retry semantic summaries and eliminate count fallback

**Files:**
- Modify: `src/status.rs`
- Test: `src/status.rs`

- [x] **Step 1: Write failing retry and terminal-error tests**

Add temporary executable integration tests that count invocations:

```rust
#[test]
fn dirty_git_status_retries_once_then_returns_summary() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(repo.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("note.txt"), "semantic change\n").unwrap();
    let cfg = test_config(dir.path());
    let attempts_path = dir.path().join("attempts");
    let codex = CodexConfig {
        bin: executable(
            dir.path().join("codex-summary"),
            &format!(
                "#!/bin/sh\nattempt=0\nif [ -f \"{}\" ]; then attempt=$(tr -d '\\n' < \"{}\"); fi\nattempt=$((attempt + 1))\nprintf '%s' \"$attempt\" > \"{}\"\nif [ \"$attempt\" -eq 1 ]; then exit 1; fi\noutput_path=''\nprevious=''\nfor argument in \"$@\"; do\n  if [ \"$previous\" = '--output-last-message' ]; then output_path=\"$argument\"; fi\n  previous=\"$argument\"\ndone\nprintf '✨ Semantic summary\\n' > \"$output_path\"\n",
                attempts_path.display(),
                attempts_path.display(),
                attempts_path.display()
            ),
        ),
        workdir: dir.path().to_path_buf(),
        default_model: "gpt-test".to_string(),
        xdg_config_home: cfg.xdg_config_home.clone(),
    };

    let got = git_status_short_summary(&codex, &cfg, "gateway", repo.path());

    assert_eq!(got, "✨ Semantic summary");
    assert_eq!(fs::read_to_string(attempts_path).unwrap(), "2");
}

#[test]
fn dirty_git_status_reports_error_after_two_failed_summaries() {
    let dir = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(repo.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success());
    fs::write(repo.path().join("note.txt"), "semantic change\n").unwrap();
    let cfg = test_config(dir.path());
    let attempts_path = dir.path().join("attempts");
    let codex = CodexConfig {
        bin: executable(
            dir.path().join("codex-summary"),
            &format!(
                "#!/bin/sh\nattempt=0\nif [ -f \"{}\" ]; then attempt=$(tr -d '\\n' < \"{}\"); fi\nattempt=$((attempt + 1))\nprintf '%s' \"$attempt\" > \"{}\"\nexit 1\n",
                attempts_path.display(),
                attempts_path.display(),
                attempts_path.display()
            ),
        ),
        workdir: dir.path().to_path_buf(),
        default_model: "gpt-test".to_string(),
        xdg_config_home: cfg.xdg_config_home.clone(),
    };

    let got = git_status_short_summary(&codex, &cfg, "gateway", repo.path());

    assert!(got.starts_with("⚠️ Summary unavailable:"));
    assert!(!got.contains("changed"));
    assert!(!got.contains("untracked"));
    assert_eq!(fs::read_to_string(attempts_path).unwrap(), "2");
}
```

Also update clean-status assertions to expect `✅ Clean`.

- [x] **Step 2: Run focused status tests and verify RED**

Run:

```bash
cargo test status::tests::dirty_git_status_
cargo test status::tests::git_status_section_
```

Expected: retry and terminal-error assertions FAIL because the current implementation calls once and falls back to counts.

- [x] **Step 3: Implement two attempts with no fallback**

Change the dirty path to build the complete diff once and attempt the same semantic summary twice:

```rust
let input = match git_summary_input(path, lines) {
    Ok(input) => input,
    Err(err) => return unavailable_git_summary(&err),
};
let prompt = git_summary_prompt(label, &input);
let light_model = cfg.light_provider_model();
let mut last_error = String::new();
for _ in 0..2 {
    match run_codex(
        codex,
        &prompt,
        None,
        light_model.provider,
        &light_model.model,
        GIT_SUMMARY_TIMEOUT.min(cfg.codex_timeout),
        &cfg.state_dir,
    ) {
        Ok(output) => match concise_git_summary(&output.final_text) {
            Some(summary) => return summary,
            None => last_error = "empty summary".to_string(),
        },
        Err(err) => last_error = err,
    }
}
unavailable_git_summary(&last_error)
```

Format terminal errors without count fallback:

```rust
fn unavailable_git_summary(error: &str) -> String {
    let detail = error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error");
    format!("⚠️ Summary unavailable: {detail}")
}
```

Remove `summarize_git_status_lines`, `GitStatusCounts`, `git_status_counts`,
`is_conflicted_status`, `push_count`, and the context-error special case. Return
`✅ Clean` when `git status --short` is empty. Keep preparation errors as
`⚠️ Summary unavailable:` followed by the first nonempty error line.

- [x] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cargo test status::tests::dirty_git_status_
cargo test status::tests::git_status_section_
```

Expected: all matching tests PASS with exactly two attempts on terminal failure.

- [x] **Step 5: Clean up without behavior changes**

Remove superseded tests and dead helpers, deduplicate temporary-repository setup only where it shortens the test module, and run:

```bash
cargo fmt -- --check
cargo test status::tests
```

Expected: formatting check and all status tests PASS.

- [x] **Step 6: Verify and commit**

Run:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
git diff --check
```

Expected: all tests pass, Clippy reports no warnings, and diff check exits successfully.

Then commit:

```bash
git add src/status.rs docs/superpowers/plans/2026-07-19-status-diff-summary.md
git commit -m "Improve status diff summaries"
```
