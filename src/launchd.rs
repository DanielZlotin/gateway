use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub const LABEL: &str = "ai.gateway";
pub const HEARTBEAT_LABEL: &str = "ai.gateway.heartbeat";

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
    Ok(target_for_uid_and_label(&uid, LABEL))
}

pub fn plist_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME is required".to_string())
        .map(|value| value.trim().to_string())?;
    plist_path_from_home(&home, LABEL)
}

pub fn plist_path_from_env(env: &BTreeMap<String, String>) -> Result<PathBuf, String> {
    let home = env
        .get("HOME")
        .map(|value| value.trim().to_string())
        .ok_or_else(|| "HOME is required".to_string())?;
    plist_path_from_home(&home, LABEL)
}

fn plist_path_from_home(home: &str, label: &str) -> Result<PathBuf, String> {
    if home.is_empty() {
        return Err("HOME is required".to_string());
    }
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{label}.plist")))
}

pub fn uninstall() -> Result<String, String> {
    let uid = uid()?;
    let home = std::env::var("HOME")
        .map_err(|_| "HOME is required".to_string())
        .map(|value| value.trim().to_string())?;
    let mut lines = Vec::new();
    for label in [LABEL, HEARTBEAT_LABEL] {
        let target = target_for_uid_and_label(&uid, label);
        let plist_path = plist_path_from_home(&home, label)?;
        lines.push(bootout_line(&target)?);
        lines.push(remove_plist_line(&plist_path)?);
    }
    Ok(lines.join("\n"))
}

fn uid() -> Result<String, String> {
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
    Ok(uid)
}

fn target_for_uid_and_label(uid: &str, label: &str) -> String {
    format!("gui/{uid}/{label}")
}

fn bootout_line(target: &str) -> Result<String, String> {
    let bootout = Command::new("/bin/launchctl")
        .args(["bootout", target])
        .status()
        .map_err(|err| format!("run launchctl bootout: {err}"))?;
    if bootout.success() {
        Ok(format!("launchd={target} booted out"))
    } else {
        Ok(format!("launchd={target} bootout exited with {bootout}"))
    }
}

fn remove_plist_line(plist_path: &PathBuf) -> Result<String, String> {
    match fs::remove_file(plist_path) {
        Ok(()) => Ok(format!("plist={} removed", plist_path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Ok(format!("plist={} already absent", plist_path.display()))
        }
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
        "ffmpeg",
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
        "arch",
        "chmod",
        "curl",
        "date",
        "id",
        "launchctl",
        "mkdir",
        "mv",
        "rm",
        "sed",
        "sleep",
        "tar",
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
        write_setup_system_tools(&stub_dir);
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
        let heartbeat_plist =
            fs::read_to_string(home.join("Library/LaunchAgents/ai.gateway.heartbeat.plist"))
                .unwrap();
        assert!(!plist.contains("EnvironmentVariables"));
        assert!(!plist.contains("GATEWAY_TELEGRAM_TOKEN"));
        assert!(!plist.contains("XDG_DATA_HOME"));
        assert!(!plist.contains("XDG_STATE_HOME"));
        assert!(plist.contains(&format!("exec {}", env!("CARGO_MANIFEST_DIR"))));
        assert!(heartbeat_plist.contains("<string>ai.gateway.heartbeat</string>"));
        assert!(heartbeat_plist.contains("<key>StartInterval</key>"));
        assert!(heartbeat_plist.contains("<integer>60</integer>"));
        assert!(!heartbeat_plist.contains("GATEWAY_HEARTBEAT_ACTIVE"));
        assert!(heartbeat_plist.contains("<string>/bin/zsh</string>"));
        assert!(heartbeat_plist.contains("<string>-lc</string>"));
        assert!(heartbeat_plist.contains(&format!(
            "<string>exec {}/target/release/gateway heartbeat</string>",
            env!("CARGO_MANIFEST_DIR")
        )));
        assert!(!heartbeat_plist.contains("__GATEWAY_HEARTBEAT_LAUNCH__"));
        let launchctl_log = fs::read_to_string(launchctl_log).unwrap();
        assert!(launchctl_log.contains("bootout"));
        assert!(launchctl_log.contains("sleep 1\nbootstrap"));
        assert!(launchctl_log.contains("ai.gateway.heartbeat"));
        assert!(
            !launchctl_log.contains("kickstart"),
            "setup should let the RunAtLoad bootstrap start the gateway once:\n{launchctl_log}"
        );
        assert!(root
            .join("home/.local/share/gateway/voicebox/Voicebox.app/Contents/MacOS/Voicebox")
            .exists());
    }

    #[test]
    fn setup_does_not_reload_heartbeat_when_running_inside_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub_dir = root.join("bin");
        let home = root.join("home");
        let launchctl_log = root.join("launchctl.log");

        fs::create_dir_all(&stub_dir).unwrap();
        for name in setup_homebrew_tool_names() {
            write_executable(&stub_dir.join(name), "#!/bin/zsh\nexit 0\n");
        }
        write_setup_system_tools(&stub_dir);
        write_executable(&stub_dir.join("jq"), "#!/bin/zsh\nprint -r -- \"$4\"\n");
        write_executable(
            &stub_dir.join("launchctl"),
            "#!/bin/zsh\nprint -- \"$*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );
        write_executable(
            &stub_dir.join("sleep"),
            "#!/bin/zsh\nprint -- \"sleep $*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );

        let output = Command::new("/bin/zsh")
            .arg(setup_script())
            .env_clear()
            .env("HOME", &home)
            .env("PATH", format!("{}:/bin:/usr/bin", stub_dir.display()))
            .env("GATEWAY_TEST_LAUNCHCTL_LOG", &launchctl_log)
            .env("GATEWAY_TELEGRAM_TOKEN", "fake-token")
            .env("GATEWAY_TELEGRAM_CHAT_ID", "42")
            .env("GATEWAY_HEARTBEAT_ACTIVE", "1")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "setup failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let launchctl_log = fs::read_to_string(launchctl_log).unwrap();
        assert!(launchctl_log.contains("ai.gateway"));
        assert!(
            !launchctl_log.contains("ai.gateway.heartbeat"),
            "heartbeat should not be reloaded while heartbeat is active:\n{launchctl_log}"
        );
        assert!(home
            .join(".local/state/gateway/suppress-startup-status.once")
            .exists());
        assert!(home
            .join("Library/LaunchAgents/ai.gateway.heartbeat.plist")
            .exists());
    }

    #[test]
    fn setup_classifies_every_required_command_for_install_or_system_path() {
        let setup = fs::read_to_string(setup_script()).unwrap();
        let required = required_commands_from_setup(&setup);

        assert_eq!(required, setup_required_tool_names());
        assert!(!setup.contains("  rust\n"));
        assert!(setup.contains("cargo|rustc) print -r -- rust ;;"));
        assert!(setup.contains("ffmpeg) print -r -- ffmpeg ;;"));
        assert!(setup.contains("rg) print -r -- ripgrep ;;"));
        assert!(setup.contains("whisper) print -r -- openai-whisper ;;"));
        assert!(setup.contains("codex) print -r -- codex ;;"));
        let cask_probe = ["brew", " info ", "--cask"].concat();
        assert!(!setup.contains(&cask_probe));
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
            .env("XDG_DATA_HOME", root.join("data"))
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
            "install rust fastfetch ffmpeg fzf gh git go jq node parallel ripgrep openai-whisper"
        ));
        assert!(brew_log.contains("install --cask codex"));
        assert!(root
            .join("data/gateway/voicebox/Voicebox.app/Contents/MacOS/Voicebox")
            .exists());
    }

    #[test]
    fn setup_refreshes_existing_voicebox_app_without_removing_models_or_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub_dir = root.join("bin");
        let home = root.join("home");
        let data_home = root.join("data");
        let voicebox_root = data_home.join("gateway/voicebox");
        let voicebox_bin = voicebox_root.join("Voicebox.app/Contents/MacOS/Voicebox");
        let model_marker = voicebox_root.join("models/model.bin");
        let data_marker = voicebox_root.join("data/profiles.json");
        let launchctl_log = root.join("launchctl.log");

        fs::create_dir_all(voicebox_bin.parent().unwrap()).unwrap();
        fs::create_dir_all(model_marker.parent().unwrap()).unwrap();
        fs::create_dir_all(data_marker.parent().unwrap()).unwrap();
        fs::write(&voicebox_bin, "#!/bin/zsh\nprint -r -- old voicebox\n").unwrap();
        fs::write(&model_marker, "model").unwrap();
        fs::write(&data_marker, "profiles").unwrap();
        fs::set_permissions(&voicebox_bin, fs::Permissions::from_mode(0o700)).unwrap();

        fs::create_dir_all(&stub_dir).unwrap();
        for name in setup_homebrew_tool_names() {
            write_executable(&stub_dir.join(name), "#!/bin/zsh\nexit 0\n");
        }
        write_setup_system_tools(&stub_dir);
        write_executable(&stub_dir.join("jq"), "#!/bin/zsh\nprint -r -- \"$4\"\n");
        write_executable(
            &stub_dir.join("launchctl"),
            "#!/bin/zsh\nprint -- \"$*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );
        write_executable(
            &stub_dir.join("sleep"),
            "#!/bin/zsh\nprint -- \"sleep $*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        );

        let output = Command::new("/bin/zsh")
            .arg(setup_script())
            .env_clear()
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &data_home)
            .env("PATH", format!("{}:/bin:/usr/bin", stub_dir.display()))
            .env("GATEWAY_TEST_LAUNCHCTL_LOG", &launchctl_log)
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
        let voicebox = fs::read_to_string(&voicebox_bin).unwrap();
        assert!(!voicebox.contains("old voicebox"));
        assert!(voicebox.contains("exit 0"));
        assert_eq!(fs::read_to_string(model_marker).unwrap(), "model");
        assert_eq!(fs::read_to_string(data_marker).unwrap(), "profiles");
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
                "arch" => "#!/bin/zsh\nprint -r -- arm64\n",
                "chmod" => "#!/bin/zsh\nexec /bin/chmod \"$@\"\n",
                "curl" => "#!/bin/zsh\nout=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [[ \"$prev\" == -o ]]; then out=\"$arg\"; fi\n  prev=\"$arg\"\ndone\nprint -r -- voicebox-archive > \"$out\"\n",
                "id" => "#!/bin/zsh\nif [[ \"${1:-}\" == -u ]]; then print -r -- 501; else exec /usr/bin/id \"$@\"; fi\n",
                "mkdir" => "#!/bin/zsh\nexec /bin/mkdir \"$@\"\n",
                "mv" => "#!/bin/zsh\nexec /bin/mv \"$@\"\n",
                "rm" => "#!/bin/zsh\nexec /bin/rm \"$@\"\n",
                "tar" => "#!/bin/zsh\ndest=\"\"\nprev=\"\"\nfor arg in \"$@\"; do\n  if [[ \"$prev\" == -C ]]; then dest=\"$arg\"; fi\n  prev=\"$arg\"\ndone\n/bin/mkdir -p \"$dest/Voicebox.app/Contents/MacOS\"\nprint -r -- '#!/bin/zsh\nexit 0' > \"$dest/Voicebox.app/Contents/MacOS/Voicebox\"\n/bin/chmod +x \"$dest/Voicebox.app/Contents/MacOS/Voicebox\"\n",
                _ => "#!/bin/zsh\nexit 0\n",
            };
            write_executable(&stub_dir.join(name), body);
        }
    }

    fn fake_brew_installer() -> &'static str {
        "#!/bin/zsh\n\
         print -r -- \"$*\" >> \"$GATEWAY_TEST_BREW_LOG\"\n\
         for gateway_arg in \"$@\"; do\n\
         \tif [[ \"$gateway_arg\" == --yes ]]; then\n\
         \t\tprint -u2 -- \"invalid option --yes\"\n\
         \t\texit 1\n\
         \tfi\n\
         done\n\
         if [[ \"${1:-}\" == install ]]; then\n\
         \tshift\n\
         \twhile [[ \"${1:-}\" == --cask ]]; do shift; done\n\
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
