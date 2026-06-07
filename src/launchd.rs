use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub const LABEL: &str = "ai.gateway";

pub fn target() -> Result<String, String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|err| format!("run id -u: {err}"))?;
    if !output.status.success() {
        return Err(format!("id -u exited with {}", output.status));
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        return Err("id -u returned empty uid".to_string());
    }
    Ok(format!("gui/{uid}/{LABEL}"))
}

pub fn plist_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME is required".to_string())
        .map(|value| value.trim().to_string())?;
    plist_path_from_home(&home)
}

pub fn plist_path_from_env(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    let home = env
        .get("HOME")
        .map(|value| value.trim().to_string())
        .ok_or_else(|| "HOME is required".to_string())?;
    plist_path_from_home(&home)
}

fn plist_path_from_home(home: &str) -> Result<PathBuf, String> {
    if home.is_empty() {
        return Err("HOME is required".to_string());
    }
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

pub fn uninstall() -> Result<String, String> {
    let target = target()?;
    let plist_path = plist_path()?;
    let bootout = Command::new("/bin/launchctl")
        .args(["bootout", &target])
        .status()
        .map_err(|err| format!("run launchctl bootout: {err}"))?;

    let bootout_line = if bootout.success() {
        format!("launchd={target} booted out")
    } else {
        format!("launchd={target} bootout exited with {bootout}")
    };

    match fs::remove_file(&plist_path) {
        Ok(()) => Ok(format!(
            "{bootout_line}\nplist={} removed",
            plist_path.display()
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(format!(
            "{bootout_line}\nplist={} already absent",
            plist_path.display()
        )),
        Err(err) => Err(format!("remove LaunchAgent plist: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    const SETUP_HOMEBREW_FORMULA_TOOLS: &[&str] = &[
        "cargo",
        "fastfetch",
        "fzf",
        "gh",
        "git",
        "go",
        "jq",
        "node",
        "parallel",
        "rg",
        "rustc",
        "whisper",
    ];
    const SETUP_HOMEBREW_CASK_TOOLS: &[&str] = &["codex"];
    const SETUP_SYSTEM_TOOLS: &[&str] = &[
        "awk",
        "date",
        "id",
        "launchctl",
        "mkdir",
        "mv",
        "rm",
        "sed",
        "sleep",
        "xargs",
    ];

    #[test]
    fn launch_script_runs_with_environment_loaded_by_zshenv() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let zdotdir = root.join("zdot");
        let state_dir = root.join("state");
        let launch_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("launch");
        let launch_dest = root.join("launch");
        let gateway_bin = root.join("target/release/gateway");

        fs::create_dir_all(&zdotdir).unwrap();
        fs::copy(launch_src, &launch_dest).unwrap();
        fs::create_dir_all(gateway_bin.parent().unwrap()).unwrap();
        fs::write(
            &gateway_bin,
            "#!/bin/zsh\nif [[ \"${1:-}\" == version ]]; then\n  print -- \"gateway 9.8.7-test\"\n  exit 0\nfi\nprint -- \"stub args=$* token=$GATEWAY_TELEGRAM_TOKEN chat=$GATEWAY_TELEGRAM_CHAT_ID state=$XDG_STATE_HOME\"\nprint -u2 -- \"stderr probe\"\n",
        )
        .unwrap();
        fs::set_permissions(&gateway_bin, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            zdotdir.join(".zshenv"),
            format!(
                "typeset -gx GATEWAY_TELEGRAM_TOKEN=token\n\
                 typeset -gx GATEWAY_TELEGRAM_CHAT_ID=42\n\
                 typeset -gx XDG_CONFIG_HOME={0}/config\n\
                 typeset -gx XDG_CACHE_HOME={0}/cache\n\
                 typeset -gx XDG_DATA_HOME={0}/data\n\
                 typeset -gx XDG_STATE_HOME={0}/state\n",
                root.display()
            ),
        )
        .unwrap();

        let launch_command = format!("exec {}", launch_dest.display());
        let output = Command::new("/bin/zsh")
            .args(["-lc", &launch_command])
            .env_clear()
            .env("HOME", root)
            .env("PATH", "/bin:/usr/bin")
            .env("ZDOTDIR", &zdotdir)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "launch failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let log = fs::read_to_string(state_dir.join("gateway/logs/gateway.log")).unwrap();
        assert_gateway_log_format(&log, "9.8.7-test");
        assert!(log.contains("ℹ️ ") && log.contains(" v=9.8.7-test starting gateway"));
        assert!(
            log.contains("ℹ️ ") && log.contains(" v=9.8.7-test stub args=bot token=token chat=42")
        );
        assert!(log.contains("❌ ") && log.contains(" v=9.8.7-test stderr probe"));
        assert!(log.contains(&format!("state={}/state", root.display())));
    }

    #[test]
    fn launch_script_uses_default_log_path_without_exporting_xdg_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let zdotdir = root.join("zdot");
        let launch_src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("launch");
        let launch_dest = root.join("launch");
        let gateway_bin = root.join("target/release/gateway");

        fs::create_dir_all(&zdotdir).unwrap();
        fs::copy(launch_src, &launch_dest).unwrap();
        fs::create_dir_all(gateway_bin.parent().unwrap()).unwrap();
        fs::write(
            &gateway_bin,
            "#!/bin/zsh\nif [[ \"${1:-}\" == version ]]; then\n  print -- \"gateway 9.8.7-test\"\n  exit 0\nfi\nprint -- \"state=${XDG_STATE_HOME-unset} config=${XDG_CONFIG_HOME-unset} cache=${XDG_CACHE_HOME-unset} data=${XDG_DATA_HOME-unset}\"\n",
        )
        .unwrap();
        fs::set_permissions(&gateway_bin, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            zdotdir.join(".zshenv"),
            "typeset -gx GATEWAY_TELEGRAM_TOKEN=token\n\
             typeset -gx GATEWAY_TELEGRAM_CHAT_ID=42\n",
        )
        .unwrap();

        let launch_command = format!("exec {}", launch_dest.display());
        let output = Command::new("/bin/zsh")
            .args(["-lc", &launch_command])
            .env_clear()
            .env("HOME", root)
            .env("PATH", "/bin:/usr/bin")
            .env("ZDOTDIR", &zdotdir)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "launch failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let log = fs::read_to_string(root.join(".local/state/gateway/logs/gateway.log")).unwrap();
        assert_gateway_log_format(&log, "9.8.7-test");
        assert!(log.contains("state=unset config=unset cache=unset data=unset"));
    }

    #[test]
    fn setup_writes_launch_path_without_runtime_environment() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub_dir = root.join("bin");
        let home = root.join("home");
        let launchctl_log = root.join("launchctl.log");

        fs::create_dir_all(&stub_dir).unwrap();
        for name in setup_homebrew_tool_names() {
            write_executable(&stub_dir.join(name), "#!/bin/zsh\nexit 0\n");
        }
        write_executable(&stub_dir.join("jq"), "#!/bin/zsh\nprint -r -- \"$4\"\n");
        write_executable(
            &stub_dir.join("launchctl"),
            "#!/bin/zsh\nprint -- \"$*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );
        write_executable(
            &stub_dir.join("sleep"),
            "#!/bin/zsh\nprint -- \"sleep $*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );

        let setup = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("setup");
        let path = format!("{}:/bin:/usr/bin", stub_dir.display());
        let output = Command::new("/bin/zsh")
            .arg(setup)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", path)
            .env("GATEWAY_TEST_LAUNCHCTL_LOG", &launchctl_log)
            .env("GATEWAY_TELEGRAM_TOKEN", "fake&token")
            .env("GATEWAY_TELEGRAM_CHAT_ID", "42,-100")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "setup failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let plist = fs::read_to_string(home.join("Library/LaunchAgents/ai.gateway.plist")).unwrap();
        assert!(!plist.contains("EnvironmentVariables"));
        assert!(!plist.contains("GATEWAY_TELEGRAM_TOKEN"));
        assert!(!plist.contains("XDG_STATE_HOME"));
        assert!(plist.contains(&format!("exec {}", env!("CARGO_MANIFEST_DIR"))));
        let launchctl_log = fs::read_to_string(launchctl_log).unwrap();
        assert!(launchctl_log.contains("bootout"));
        assert!(launchctl_log.contains("sleep 1\nbootstrap"));
        assert!(launchctl_log.contains("sleep 1\nkickstart"));
    }

    #[test]
    fn setup_classifies_every_required_command_for_install_or_system_path() {
        let setup = fs::read_to_string(setup_script()).unwrap();
        let required = required_commands_from_setup(&setup);

        assert_eq!(required, setup_required_tool_names());
        assert!(!setup.contains("  rust\n"));
        assert!(setup.contains("cargo|rustc) print -r -- rust ;;"));
        assert!(setup.contains("rg) print -r -- ripgrep ;;"));
        assert!(setup.contains("whisper) print -r -- openai-whisper ;;"));
        assert!(setup.contains("codex) print -r -- codex ;;"));
    }

    #[test]
    fn setup_installs_all_missing_homebrew_tools() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub_dir = root.join("bin");
        let home = root.join("home");
        let launchctl_log = root.join("launchctl.log");
        let brew_log = root.join("brew.log");

        fs::create_dir_all(&stub_dir).unwrap();
        write_setup_system_tools(&stub_dir);
        write_executable(&stub_dir.join("brew"), fake_brew_installer());
        write_executable(
            &stub_dir.join("launchctl"),
            "#!/bin/zsh\nprint -- \"$*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );
        write_executable(
            &stub_dir.join("sleep"),
            "#!/bin/zsh\nprint -- \"sleep $*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );

        let path = stub_dir.display().to_string();
        let output = Command::new("/bin/zsh")
            .arg(setup_script())
            .env_clear()
            .env("HOME", &home)
            .env("PATH", path)
            .env("GATEWAY_TEST_BREW_LOG", &brew_log)
            .env("GATEWAY_TEST_LAUNCHCTL_LOG", &launchctl_log)
            .env("GATEWAY_TEST_STUB_DIR", &stub_dir)
            .env("GATEWAY_TELEGRAM_TOKEN", "fake-token")
            .env("GATEWAY_TELEGRAM_CHAT_ID", "42")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "setup failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let brew_log = fs::read_to_string(brew_log).unwrap();
        assert!(brew_log.contains(
            "install rust fastfetch fzf gh git go jq node parallel ripgrep openai-whisper"
        ));
        assert!(brew_log.contains("install --cask codex"));
    }

    fn setup_script() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("setup")
    }

    fn setup_required_tool_names() -> BTreeSet<&'static str> {
        SETUP_HOMEBREW_FORMULA_TOOLS
            .iter()
            .chain(SETUP_HOMEBREW_CASK_TOOLS)
            .chain(SETUP_SYSTEM_TOOLS)
            .copied()
            .collect()
    }

    fn setup_homebrew_tool_names() -> BTreeSet<&'static str> {
        SETUP_HOMEBREW_FORMULA_TOOLS
            .iter()
            .chain(SETUP_HOMEBREW_CASK_TOOLS)
            .copied()
            .collect()
    }

    fn write_setup_system_tools(stub_dir: &Path) {
        for name in SETUP_SYSTEM_TOOLS {
            let body = match *name {
                "id" => "#!/bin/zsh\nif [[ \"${1:-}\" == -u ]]; then print -r -- 501; else exec /usr/bin/id \"$@\"; fi\n",
                "mkdir" => "#!/bin/zsh\nexec /bin/mkdir \"$@\"\n",
                "mv" => "#!/bin/zsh\nexec /bin/mv \"$@\"\n",
                "rm" => "#!/bin/zsh\nexec /bin/rm \"$@\"\n",
                _ => "#!/bin/zsh\nexit 0\n",
            };
            write_executable(&stub_dir.join(name), body);
        }
    }

    fn fake_brew_installer() -> &'static str {
        "#!/bin/zsh\n\
         print -r -- \"$*\" >> \"$GATEWAY_TEST_BREW_LOG\"\n\
         if [[ \"${1:-}\" == install ]]; then\n\
         \tshift\n\
         \t[[ \"${1:-}\" == --cask ]] && shift\n\
         \tfor gateway_formula in \"$@\"; do\n\
         \t\tcase \"$gateway_formula\" in\n\
         \t\t\trust) print -r -- '#!/bin/zsh\nexit 0' > \"$GATEWAY_TEST_STUB_DIR/cargo\"; /bin/chmod +x \"$GATEWAY_TEST_STUB_DIR/cargo\"; gateway_command=rustc ;;\n\
         \t\t\tripgrep) gateway_command=rg ;;\n\
         \t\t\topenai-whisper) gateway_command=whisper ;;\n\
         \t\t\t*) gateway_command=\"$gateway_formula\" ;;\n\
         \t\tesac\n\
         \t\tprint -r -- '#!/bin/zsh\nexit 0' > \"$GATEWAY_TEST_STUB_DIR/$gateway_command\"\n\
         \t\t/bin/chmod +x \"$GATEWAY_TEST_STUB_DIR/$gateway_command\"\n\
         \tdone\n\
         fi\n\
         exit 0\n"
    }

    fn required_commands_from_setup(setup: &str) -> BTreeSet<&str> {
        let start = setup
            .find("gateway_required_commands=(")
            .expect("setup should define gateway_required_commands");
        let rest = &setup[start..];
        let body = rest
            .split_once('(')
            .and_then(|(_, value)| value.split_once(')'))
            .map(|(value, _)| value)
            .expect("gateway_required_commands should be a parenthesized zsh array");
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect()
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn assert_gateway_log_format(log: &str, version: &str) {
        let expected = format!(" v={version} ");
        assert!(
            log.lines().all(|line| {
                let Some((icon, rest)) = line.split_once(' ') else {
                    return false;
                };
                let bytes = rest.as_bytes();
                !icon.is_empty()
                    && !icon.chars().all(|ch| ch.is_ascii_alphanumeric())
                    && !line.contains("gateway version=")
                    && !line.contains("level=")
                    && !line.contains("icon=")
                    && rest.contains(&expected)
                    && bytes.get(10) == Some(&b' ')
                    && bytes.get(19) == Some(&b' ')
            }),
            "log lines did not use the compact gateway envelope:\n{log}"
        );
    }
}
