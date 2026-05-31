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
