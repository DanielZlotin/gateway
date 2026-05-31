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
    use std::os::unix::fs::PermissionsExt;

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
            "#!/bin/zsh\nprint -- \"stub args=$* token=$GATEWAY_TELEGRAM_TOKEN chats=$GATEWAY_TELEGRAM_CHAT_IDS state=$XDG_STATE_HOME\"\n",
        )
        .unwrap();
        fs::set_permissions(&gateway_bin, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            zdotdir.join(".zshenv"),
            format!(
                "typeset -gx GATEWAY_TELEGRAM_TOKEN=token\n\
                 typeset -gx GATEWAY_TELEGRAM_CHAT_IDS=42\n\
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
        assert!(log.contains("stub args=bot token=token chats=42"));
    }

    #[test]
    fn setup_writes_launch_path_without_runtime_environment() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stub_dir = root.join("bin");
        let home = root.join("home");
        let config = root.join("config");
        let cache = root.join("cache");
        let data = root.join("data");
        let state = root.join("state");
        let launchctl_log = root.join("launchctl.log");

        fs::create_dir_all(&stub_dir).unwrap();
        for name in ["cargo", "codex", "fastfetch"] {
            let path = stub_dir.join(name);
            fs::write(&path, "#!/bin/zsh\nexit 0\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let launchctl = stub_dir.join("launchctl");
        fs::write(
            &launchctl,
            "#!/bin/zsh\nprint -- \"$*\" >> \"$GATEWAY_TEST_LAUNCHCTL_LOG\"\nexit 0\n",
        )
        .unwrap();
        fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o700)).unwrap();

        let setup = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("setup");
        let path = format!(
            "{}:{}",
            stub_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let output = Command::new("/bin/zsh")
            .arg(setup)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", path)
            .env("GATEWAY_TEST_LAUNCHCTL_LOG", &launchctl_log)
            .env("GATEWAY_TELEGRAM_TOKEN", "fake&token")
            .env("GATEWAY_TELEGRAM_CHAT_IDS", "42,-100")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_CACHE_HOME", &cache)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", &state)
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
        assert!(fs::read_to_string(launchctl_log)
            .unwrap()
            .contains("bootstrap"));
    }
}
