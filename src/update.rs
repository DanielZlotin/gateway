use crate::config::Config;
use crate::logs;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const GATEWAY_UPDATE_JOB_LABEL: &str = "ai.gateway.update";
const GATEWAY_UPDATE_PENDING_LOCK_TTL_SECS: u64 = 300;
const FOUNDRY_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/foundry-rs/foundry/refs/heads/master/foundryup/foundryup";

const GATEWAY_UPDATE_SCRIPT: &str = r#"gateway_update_label="$1"
gateway_update_lock="$2"
gateway_update_root="$3"
gateway_foundry_installer_url="$4"
gateway_update_version="$5"
gateway_update_log="${gateway_update_lock:h}/logs/gateway.log"
set -o pipefail
mkdir -p "${gateway_update_log:h}"
print -r -- "pid $$" > "$gateway_update_lock"
gateway_log() {
  print -r -- "$1 $(date -u '+%Y-%m-%d %H:%M:%S') v=$gateway_update_version $2" >>"$gateway_update_log"
}
gateway_step() {
  gateway_update_phase="$1"
  shift
  gateway_log "ℹ️" "📦 update $gateway_update_phase"
  "$@" >/dev/null 2>&1
}
gateway_log "ℹ️" "📦 update start"
cd "$gateway_update_root" &&
  export HOMEBREW_NO_ASK=1 &&
  gateway_step git git pull &&
  gateway_step brew-update brew update &&
  gateway_step brew-upgrade brew upgrade --yes &&
  gateway_step brew-cleanup brew cleanup &&
  gateway_log "ℹ️" "📦 update brewsave" &&
  : "${XDG_CONFIG_HOME:?XDG_CONFIG_HOME is required}" &&
  gateway_brewfile="$XDG_CONFIG_HOME/homebrew/Brewfile" &&
  mkdir -p "${gateway_brewfile:h}" &&
  HOMEBREW_BUNDLE_FILE_GLOBAL="$gateway_brewfile" brew bundle dump --global --force --describe >/dev/null 2>&1 &&
  gateway_log "ℹ️" "📦 update foundry" &&
  (curl -sSfL "$gateway_foundry_installer_url" | bash) >/dev/null 2>&1 &&
  gateway_step setup ./setup
gateway_update_code=$?
if [[ "$gateway_update_code" -eq 0 ]]; then
  gateway_log "ℹ️" "📦 update done"
else
  gateway_log "❌" "📦 update failed code=$gateway_update_code"
fi
rm -f "$gateway_update_lock"
[[ -z "$gateway_update_label" ]] || /bin/launchctl remove "$gateway_update_label" >/dev/null 2>&1 || true
exit "$gateway_update_code""#;

#[derive(Debug, PartialEq, Eq)]
pub enum GatewayUpdateRun {
    Completed,
    AlreadyRunning,
}

#[derive(Debug, PartialEq, Eq)]
pub enum GatewayUpdateStart {
    Started,
    AlreadyRunning,
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayUpdateLockAcquire {
    Acquired,
    AlreadyRunning,
}

#[derive(Debug, PartialEq, Eq)]
enum GatewayUpdateLockStatus {
    Absent,
    Active,
    Stale,
}

pub fn start_gateway_update(cfg: &Config) -> Result<GatewayUpdateStart, String> {
    let lock_file = gateway_update_lock_file(cfg);
    if !acquire_gateway_update_lock(&lock_file)? {
        return Ok(GatewayUpdateStart::AlreadyRunning);
    }
    logs::info(format_args!(
        "📦 update pending lock={}",
        lock_file.display()
    ));

    if let Err(err) = submit_gateway_update(&lock_file) {
        let _ = fs::remove_file(&lock_file);
        return Err(err);
    }
    logs::info(format_args!(
        "📦 update submitted lock={}",
        lock_file.display()
    ));

    Ok(GatewayUpdateStart::Started)
}

pub fn run_gateway_update_inline(cfg: &Config) -> Result<GatewayUpdateRun, String> {
    let lock_file = gateway_update_lock_file(cfg);
    if !acquire_gateway_update_lock(&lock_file)? {
        return Ok(GatewayUpdateRun::AlreadyRunning);
    }

    let run_result = gateway_update_script_command(&lock_file, None)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("run gateway update: {err}"));
    let run_status = match run_result {
        Ok(run_status) => run_status,
        Err(err) => {
            let _ = fs::remove_file(&lock_file);
            return Err(err);
        }
    };
    if run_status.success() {
        Ok(GatewayUpdateRun::Completed)
    } else {
        Err(format!("gateway update exited with {run_status}"))
    }
}

pub(crate) fn gateway_update_lock_file(cfg: &Config) -> PathBuf {
    cfg.state_dir.join("update.lock")
}

fn gateway_update_lock_status(lock_file: &Path) -> Result<GatewayUpdateLockStatus, String> {
    let text = match fs::read_to_string(lock_file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(GatewayUpdateLockStatus::Absent);
        }
        Err(err) => return Err(format!("read update lock: {err}")),
    };
    let mut parts = text.split_whitespace();
    let kind = parts.next().unwrap_or_default();
    let value = parts.next().unwrap_or_default();

    match kind {
        "pid" => Ok(value
            .parse::<u32>()
            .ok()
            .filter(|pid| process_is_running(*pid))
            .map(|_| GatewayUpdateLockStatus::Active)
            .unwrap_or(GatewayUpdateLockStatus::Stale)),
        "pending" => Ok(value
            .parse::<u64>()
            .ok()
            .filter(|seconds| {
                current_unix_seconds().saturating_sub(*seconds)
                    < GATEWAY_UPDATE_PENDING_LOCK_TTL_SECS
            })
            .map(|_| GatewayUpdateLockStatus::Active)
            .unwrap_or(GatewayUpdateLockStatus::Stale)),
        _ => Ok(GatewayUpdateLockStatus::Stale),
    }
}

fn acquire_gateway_update_lock(lock_file: &Path) -> Result<bool, String> {
    match try_acquire_gateway_update_lock(lock_file)? {
        GatewayUpdateLockAcquire::Acquired => Ok(true),
        GatewayUpdateLockAcquire::AlreadyRunning => {
            logs::warn(format_args!(
                "📦 update active lock={}",
                lock_file.display()
            ));
            Ok(false)
        }
    }
}

fn try_acquire_gateway_update_lock(lock_file: &Path) -> Result<GatewayUpdateLockAcquire, String> {
    loop {
        if let Some(parent) = lock_file.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create update state dir: {err}"))?;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_file)
        {
            Ok(mut file) => {
                if let Err(err) = writeln!(file, "pending {}", current_unix_seconds()) {
                    let _ = fs::remove_file(lock_file);
                    return Err(format!("write update lock: {err}"));
                }
                return Ok(GatewayUpdateLockAcquire::Acquired);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                match gateway_update_lock_status(lock_file)? {
                    GatewayUpdateLockStatus::Active => {
                        return Ok(GatewayUpdateLockAcquire::AlreadyRunning);
                    }
                    GatewayUpdateLockStatus::Stale => {
                        logs::warn(format_args!("📦 update stale lock={}", lock_file.display()));
                        remove_gateway_update_lock(lock_file)?;
                    }
                    GatewayUpdateLockStatus::Absent => {}
                }
            }
            Err(err) => return Err(format!("create update lock: {err}")),
        }
    }
}

fn remove_gateway_update_lock(lock_file: &Path) -> Result<(), String> {
    match fs::remove_file(lock_file) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove update lock: {err}")),
    }
}

fn process_is_running(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|command_status| command_status.success())
        .unwrap_or(false)
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn gateway_update_command(lock_file: &Path) -> Command {
    let mut command = Command::new("/bin/launchctl");
    command
        .args([
            "submit",
            "-l",
            GATEWAY_UPDATE_JOB_LABEL,
            "-o",
            "/dev/null",
            "-e",
            "/dev/null",
            "--",
            "/bin/zsh",
            "-lc",
            GATEWAY_UPDATE_SCRIPT,
            "gateway-update",
            GATEWAY_UPDATE_JOB_LABEL,
        ])
        .arg(lock_file)
        .arg(gateway_root())
        .arg(FOUNDRY_INSTALLER_URL)
        .arg(env!("CARGO_PKG_VERSION"));
    command
}

#[cfg(not(test))]
fn submit_gateway_update(lock_file: &Path) -> Result<(), String> {
    let _ = Command::new("/bin/launchctl")
        .args(["remove", GATEWAY_UPDATE_JOB_LABEL])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let status = gateway_update_command(lock_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("run launchctl submit: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("launchctl submit exited with {status}"))
    }
}

#[cfg(test)]
fn submit_gateway_update(_lock_file: &Path) -> Result<(), String> {
    Ok(())
}

fn gateway_update_script_command(lock_file: &Path, label: Option<&str>) -> Command {
    let mut command = Command::new("/bin/zsh");
    command
        .args([
            "-lc",
            GATEWAY_UPDATE_SCRIPT,
            "gateway-update",
            label.unwrap_or_default(),
        ])
        .arg(lock_file)
        .arg(gateway_root())
        .arg(FOUNDRY_INSTALLER_URL)
        .arg(env!("CARGO_PKG_VERSION"));
    command
}

fn gateway_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn gateway_update_command_submits_stable_launchd_job_with_lock_cleanup() {
        let lock_file = Path::new("/tmp/gateway-state/update.lock");
        let command = gateway_update_command(lock_file);
        let args = command.get_args().collect::<Vec<_>>();

        assert_eq!(command.get_program(), OsStr::new("/bin/launchctl"));
        assert_eq!(args[0], OsStr::new("submit"));
        assert!(args
            .iter()
            .any(|arg| *arg == OsStr::new("ai.gateway.update")));

        let script = args[10].to_string_lossy();
        assert!(script.contains("gateway_update_label=\"$1\""));
        assert!(script.contains("gateway_update_lock=\"$2\""));
        assert!(script.contains("gateway_update_root=\"$3\""));
        assert!(script.contains("print -r -- \"pid $$\" > \"$gateway_update_lock\""));
        assert!(script.contains("export HOMEBREW_NO_ASK=1"));
        assert!(script.contains("brew upgrade --yes"));
        assert!(script.contains("gateway_brewfile=\"$XDG_CONFIG_HOME/homebrew/Brewfile\""));
        assert!(script.contains("mkdir -p \"${gateway_brewfile:h}\""));
        assert!(script.contains(
            "HOMEBREW_BUNDLE_FILE_GLOBAL=\"$gateway_brewfile\" brew bundle dump --global --force --describe"
        ));
        assert!(script.contains("gateway_foundry_installer_url=\"$4\""));
        assert!(script.contains("📦 update foundry"));
        assert!(script.contains("curl -sSfL \"$gateway_foundry_installer_url\" | bash"));
        assert!(script.contains("./setup"));
        assert!(script.contains("rm -f \"$gateway_update_lock\""));
        assert!(script.contains(
            "[[ -z \"$gateway_update_label\" ]] || /bin/launchctl remove \"$gateway_update_label\""
        ));
        assert!(script.contains("exit \"$gateway_update_code\""));
        assert!(script.contains("gateway_update_version=\"$5\""));
        assert!(script.contains("gateway_update_log=\"${gateway_update_lock:h}/logs/gateway.log\""));
        assert!(!script.contains("logs/update.log"));
        assert!(args
            .iter()
            .any(|arg| *arg == OsStr::new(FOUNDRY_INSTALLER_URL)));
        let brew_cleanup = script.find("brew cleanup").unwrap();
        let brewsave = script
            .find("brew bundle dump --global --force --describe")
            .unwrap();
        let foundry_update = script.find("📦 update foundry").unwrap();
        assert!(brew_cleanup < brewsave);
        assert!(brewsave < foundry_update);
    }

    #[test]
    fn gateway_update_lock_acquisition_is_atomic() {
        let dir = tempdir().unwrap();
        let lock_file = dir.path().join("update.lock");

        assert_eq!(
            try_acquire_gateway_update_lock(&lock_file).unwrap(),
            GatewayUpdateLockAcquire::Acquired
        );
        assert_eq!(
            try_acquire_gateway_update_lock(&lock_file).unwrap(),
            GatewayUpdateLockAcquire::AlreadyRunning
        );
        assert!(fs::read_to_string(lock_file)
            .unwrap()
            .starts_with("pending "));
    }
}
