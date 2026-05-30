use crate::text::session_label;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub model: String,
    #[serde(default)]
    pub generation: i64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SavedSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub model: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub enum SessionKey {
    Chat {
        chat_id: i64,
        thread_id: Option<i64>,
    },
    Cron(String),
}

pub struct SessionStore {
    chat_dir: PathBuf,
    cron_dir: PathBuf,
    default_model: String,
}

impl SessionStore {
    pub fn new(chat_dir: PathBuf, cron_dir: PathBuf, default_model: String) -> Self {
        Self {
            chat_dir,
            cron_dir,
            default_model,
        }
    }

    pub fn load(&self, key: &SessionKey) -> ChatSession {
        self.load_path(&self.path(key))
    }

    pub fn reset(&self, key: &SessionKey) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        state.session_id = None;
        state.generation += 1;
        state.updated_at = now_string();
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn set_model(&self, key: &SessionKey, model: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        state.model = model.trim().to_string();
        state.updated_at = now_string();
        if let Some(id) = state.session_id.clone() {
            state.sessions = upsert_session(
                state.sessions,
                SavedSession {
                    id,
                    name: None,
                    model: state.model.clone(),
                    updated_at: state.updated_at.clone(),
                },
                &self.default_model,
            );
        }
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn resume(&self, key: &SessionKey, target: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let found = find_session(&state.sessions, target)
            .ok_or_else(|| format!("No saved session matches \"{target}\"."))?;
        state.session_id = Some(found.id);
        if !found.model.is_empty() {
            state.model = found.model;
        }
        state.generation += 1;
        state.updated_at = now_string();
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn rename_current(&self, key: &SessionKey, name: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let id = state.session_id.clone().ok_or_else(|| {
            "No current session to rename. Send a normal message first.".to_string()
        })?;
        state.updated_at = now_string();
        state.sessions = upsert_session(
            state.sessions,
            SavedSession {
                id,
                name: Some(name.trim().to_string()),
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            },
            &self.default_model,
        );
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn save_run(
        &self,
        key: &SessionKey,
        expected_generation: i64,
        session_id: &str,
    ) -> Result<bool, String> {
        let mut state = self.load(key);
        if state.generation != expected_generation {
            return Ok(false);
        }
        state.session_id = Some(session_id.to_string());
        state.updated_at = now_string();
        state.sessions = upsert_session(
            state.sessions,
            SavedSession {
                id: session_id.to_string(),
                name: None,
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            },
            &self.default_model,
        );
        self.save(key, &state)?;
        Ok(true)
    }

    pub fn list(&self, key: &SessionKey) -> String {
        let state = self.load(key);
        if state.sessions.is_empty() {
            return "No saved sessions yet. Send a normal message to create one.".to_string();
        }
        let mut lines = vec!["Saved sessions:".to_string()];
        for item in state.sessions {
            let marker = if Some(item.id.as_str()) == state.session_id.as_deref() {
                "*"
            } else {
                " "
            };
            let name = item.name.as_deref().unwrap_or("(unnamed)");
            let model = if item.model.is_empty() {
                self.default_model.as_str()
            } else {
                item.model.as_str()
            };
            lines.push(format!(
                "{marker} {} {model} {name}",
                session_label(&item.id)
            ));
        }
        lines.join("\n")
    }

    fn path(&self, key: &SessionKey) -> PathBuf {
        match key {
            SessionKey::Chat { chat_id, thread_id } => {
                let suffix = thread_id
                    .map(|id| format!("thread-{id}"))
                    .unwrap_or_else(|| "main".to_string());
                self.chat_dir.join(format!("{chat_id}-{suffix}.json"))
            }
            SessionKey::Cron(name) => self.cron_dir.join(format!("{}.json", sanitize_key(name))),
        }
    }

    fn load_path(&self, path: &Path) -> ChatSession {
        let mut state = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<ChatSession>(&text).ok())
            .unwrap_or_else(|| ChatSession {
                model: self.default_model.clone(),
                ..ChatSession::default()
            });
        if state.model.trim().is_empty() {
            state.model = self.default_model.clone();
        }
        if state.session_id.is_some() && state.sessions.is_empty() {
            state.sessions.push(SavedSession {
                id: state.session_id.clone().unwrap_or_default(),
                name: None,
                model: state.model.clone(),
                updated_at: state.updated_at.clone(),
            });
        }
        state
    }

    fn save(&self, key: &SessionKey, state: &ChatSession) -> Result<(), String> {
        let path = self.path(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create session dir: {err}"))?;
        }
        let data = serde_json::to_vec_pretty(state).map_err(|err| err.to_string())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, [data, b"\n".to_vec()].concat()).map_err(|err| err.to_string())?;
        fs::rename(&tmp, &path).map_err(|err| err.to_string())
    }
}

pub fn upsert_session(
    mut items: Vec<SavedSession>,
    mut item: SavedSession,
    default_model: &str,
) -> Vec<SavedSession> {
    item.id = item.id.trim().to_string();
    if item.id.is_empty() {
        return items;
    }
    if item.model.trim().is_empty() {
        item.model = default_model.to_string();
    }
    for existing in &mut items {
        if existing.id == item.id {
            if item.name.is_none() {
                item.name = existing.name.clone();
            }
            *existing = item;
            return items;
        }
    }
    let mut out = vec![item];
    out.extend(items);
    out
}

pub fn find_session(items: &[SavedSession], target: &str) -> Option<SavedSession> {
    let target = target.trim();
    items
        .iter()
        .find(|item| {
            item.id == target
                || session_label(&item.id) == target
                || item.name.as_deref() == Some(target)
        })
        .cloned()
}

fn sanitize_key(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn upsert_preserves_existing_name_and_finds_by_name_or_short_id() {
        let first = SavedSession {
            id: "019e778b-2c3f-7231-bda6-c40f27bbba21".to_string(),
            name: Some("main".to_string()),
            model: "gpt-5.5".to_string(),
            updated_at: "now".to_string(),
        };
        let second = SavedSession {
            id: first.id.clone(),
            name: None,
            model: "gpt-test".to_string(),
            updated_at: "later".to_string(),
        };

        let items = upsert_session(vec![first], second, "gpt-5.5");

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name.as_deref(), Some("main"));
        assert_eq!(items[0].model, "gpt-test");
        assert!(find_session(&items, "main").is_some());
        assert!(find_session(&items, "019e778b").is_some());
    }

    #[test]
    fn reset_clears_session_and_increments_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(
            dir.path().join("chats"),
            dir.path().join("cron"),
            "gpt-5.5".to_string(),
        );
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };

        assert_eq!(store.reset(&key).unwrap().generation, 1);
        let loaded = store.load(&key);

        assert_eq!(loaded.session_id, None);
        assert_eq!(loaded.model, "gpt-5.5");
        assert_eq!(loaded.generation, 1);
    }

    #[test]
    fn save_run_rejects_stale_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(
            dir.path().join("chats"),
            dir.path().join("cron"),
            "gpt-5.5".to_string(),
        );
        let key = SessionKey::Cron("daily".to_string());

        store.reset(&key).unwrap();
        assert!(!store.save_run(&key, 0, "stale").unwrap());
        assert!(store.save_run(&key, 1, "fresh").unwrap());
        assert_eq!(store.load(&key).session_id.as_deref(), Some("fresh"));
    }
}
