use crate::config::ProviderModel;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize)]
pub struct ExecutionState {
    id: String,
    pid: u32,
    pub running: bool,
    pub configured: ProviderModel,
    pub model: Option<String>,
    pub last_used: Option<ProviderModel>,
}

pub fn read_execution(path: &Path) -> Option<ExecutionState> {
    let mut state: ExecutionState = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if state.running {
        state.running = Command::new("/bin/kill")
            .args(["-0", &state.pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    }
    Some(state)
}

// The lock covers the identity check and atomic replacement across threads/processes.
fn lock_execution(path: &Path) -> Result<File, String> {
    fs::create_dir_all(path.parent().unwrap()).map_err(|err| err.to_string())?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|err| err.to_string())?;
    lock.lock().map_err(|err| err.to_string())?;
    Ok(lock)
}

fn save_execution(path: &Path, state: &ExecutionState) -> Result<(), String> {
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|err| err.to_string())?;
    file.write_all(&serde_json::to_vec(state).map_err(|err| err.to_string())?)
        .map_err(|err| err.to_string())?;
    file.persist(path).map_err(|err| err.to_string())?;
    Ok(())
}

pub struct Execution {
    path: PathBuf,
    id: String,
}

impl Execution {
    pub fn begin(path: &Path, configured: ProviderModel) -> Result<Self, String> {
        let _lock = lock_execution(path)?;
        let token = tempfile::NamedTempFile::new_in(path.parent().unwrap())
            .map_err(|err| err.to_string())?;
        let id = token
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let last_used = read_execution(path).and_then(|state| state.last_used);
        save_execution(
            path,
            &ExecutionState {
                id: id.clone(),
                pid: std::process::id(),
                running: true,
                configured,
                model: None,
                last_used,
            },
        )?;
        Ok(Self {
            path: path.to_path_buf(),
            id,
        })
    }

    pub fn report_model(&self, model: &str) -> Result<(), String> {
        self.update(|state| {
            state.model = Some(model.to_string());
            state.last_used = Some(ProviderModel {
                provider: state.configured.provider,
                model: model.to_string(),
            });
        })
    }

    fn update(&self, update: impl FnOnce(&mut ExecutionState)) -> Result<(), String> {
        let _lock = lock_execution(&self.path)?;
        if let Some(mut state) =
            read_execution(&self.path).filter(|state| state.id == self.id && state.running)
        {
            update(&mut state);
            save_execution(&self.path, &state)?;
        }
        Ok(())
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        if let Err(err) = self.update(|state| state.running = false) {
            crate::logs::warn(format_args!("finish execution metadata: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;

    fn choice(model: &str) -> ProviderModel {
        ProviderModel {
            provider: Provider::Codex,
            model: model.into(),
        }
    }

    #[test]
    fn execution_is_unknown_until_reported_and_idle_after_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        let run = Execution::begin(&path, choice("")).unwrap();
        let state = read_execution(&path).unwrap();
        assert!(state.running);
        assert_eq!(state.model, None);
        assert_eq!(state.configured, choice(""));
        run.report_model("gpt-6-astra").unwrap();
        assert_eq!(
            read_execution(&path).unwrap().model.as_deref(),
            Some("gpt-6-astra")
        );
        drop(run);
        let state = read_execution(&path).unwrap();
        assert!(!state.running);
        assert_eq!(state.last_used, Some(choice("gpt-6-astra")));
    }

    #[test]
    fn superseded_run_cannot_change_newer_execution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        let old = Execution::begin(&path, choice("")).unwrap();
        old.report_model("old").unwrap();
        let new = Execution::begin(&path, choice("override")).unwrap();
        new.report_model("new").unwrap();
        old.report_model("late").unwrap();
        drop(old);
        let state = read_execution(&path).unwrap();
        assert!(state.running);
        assert_eq!(state.model.as_deref(), Some("new"));
        assert_eq!(state.configured, choice("override"));
        drop(new);
        assert!(!read_execution(&path).unwrap().running);
    }

    #[test]
    fn dead_owner_is_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        let _run = Execution::begin(&path, choice("")).unwrap();
        let mut state = read_execution(&path).unwrap();
        state.pid = u32::MAX;
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(!read_execution(&path).unwrap().running);
    }
}
