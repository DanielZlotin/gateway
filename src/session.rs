use crate::json_file::{save_pretty_json, SaveJsonLabels};
use crate::provider::Provider;
use crate::text::session_label;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub provider: Provider,
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
    pub provider: Provider,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SessionKey {
    Chat {
        chat_id: i64,
        thread_id: Option<i64>,
    },
}

pub struct SessionStore {
    chat_dir: PathBuf,
    default_model: String,
    default_provider: Provider,
}

impl SessionStore {
    pub const fn new(chat_dir: PathBuf, default_model: String) -> Self {
        Self::new_with_provider(chat_dir, default_model, Provider::Codex)
    }

    pub const fn new_with_provider(
        chat_dir: PathBuf,
        default_model: String,
        default_provider: Provider,
    ) -> Self {
        Self {
            chat_dir,
            default_model,
            default_provider,
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

    pub fn set_provider(
        &self,
        key: &SessionKey,
        provider: Provider,
        model: &str,
    ) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        state.provider = provider;
        state.model = model.trim().to_string();
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
                    provider: state.provider,
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
            .ok_or_else(|| format!("🔎 No saved session matches \"{target}\"."))?;
        apply_resumed_session(&mut state, found);
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn resume_relative(&self, key: &SessionKey, back: usize) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let current_id = state
            .session_id
            .as_deref()
            .ok_or_else(|| "🔎 No current session to step back from.".to_string())?;
        let current_index = state
            .sessions
            .iter()
            .position(|item| item.id == current_id)
            .ok_or_else(|| "🔎 Current session is not in saved sessions.".to_string())?;
        let target_index = current_index + back;
        let found = state
            .sessions
            .get(target_index)
            .cloned()
            .ok_or_else(|| format!("🔎 No saved session {back} sessions back."))?;
        apply_resumed_session(&mut state, found);
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn rename_current(&self, key: &SessionKey, name: &str) -> Result<ChatSession, String> {
        let mut state = self.load(key);
        let id = state.session_id.clone().ok_or_else(|| {
            "🏷️ No current session to rename. Send a normal message first.".to_string()
        })?;
        state.updated_at = now_string();
        state.sessions = upsert_session(
            state.sessions,
            SavedSession {
                id,
                name: Some(name.trim().to_string()),
                model: state.model.clone(),
                provider: state.provider,
                updated_at: state.updated_at.clone(),
            },
            &self.default_model,
        );
        self.save(key, &state)?;
        Ok(state)
    }

    pub fn save_current_session(
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
                provider: state.provider,
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
            return "📭 No saved sessions yet. Send a normal message to create one.".to_string();
        }
        let mut lines = vec!["💾 Saved sessions:".to_string()];
        for item in state.sessions {
            let marker = if Some(item.id.as_str()) == state.session_id.as_deref() {
                "⭐"
            } else {
                "▫️"
            };
            let name = item.name.as_deref().unwrap_or("(unnamed)");
            let model = if item.model.is_empty() {
                self.default_model.as_str()
            } else {
                item.model.as_str()
            };
            lines.push(format!(
                "{marker} {} {} {model} {name}",
                item.provider.label(),
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
        }
    }

    fn load_path(&self, path: &Path) -> ChatSession {
        let mut state = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<ChatSession>(&text).ok())
            .unwrap_or_else(|| ChatSession {
                provider: self.default_provider,
                model: self.default_model.clone(),
                ..ChatSession::default()
            });
        if state.model.trim().is_empty() {
            state.model = self.default_model.clone();
        }
        state
    }

    fn save(&self, key: &SessionKey, state: &ChatSession) -> Result<(), String> {
        let path = self.path(key);
        save_pretty_json(
            &path,
            state,
            SaveJsonLabels {
                create_dir: "create session dir",
                write: "write session",
                replace: "replace session",
            },
        )
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

fn apply_resumed_session(state: &mut ChatSession, found: SavedSession) {
    state.session_id = Some(found.id);
    if !found.model.is_empty() {
        state.model = found.model;
    }
    state.provider = found.provider;
    state.generation += 1;
    state.updated_at = now_string();
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
            provider: Provider::Codex,
            updated_at: "now".to_string(),
        };
        let second = SavedSession {
            id: first.id.clone(),
            name: None,
            model: "gpt-test".to_string(),
            provider: Provider::Codex,
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
        let store = SessionStore::new(dir.path().join("chats"), "gpt-5.5".to_string());
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
    fn save_current_session_rejects_stale_generation() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-5.5".to_string());
        let key = SessionKey::Chat {
            chat_id: 42,
            thread_id: None,
        };

        store.reset(&key).unwrap();
        assert!(!store.save_current_session(&key, 0, "stale").unwrap());
        assert!(store.save_current_session(&key, 1, "fresh").unwrap());
        assert_eq!(store.load(&key).session_id.as_deref(), Some("fresh"));
    }

    #[test]
    fn set_model_updates_current_saved_session_and_trims_model() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-default".to_string());
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: Some(99),
        };

        assert!(store
            .save_current_session(&key, 0, "session-12345678")
            .unwrap());
        let state = store.set_model(&key, " gpt-new ").unwrap();

        assert_eq!(state.model, "gpt-new");
        assert_eq!(state.sessions.len(), 1);
        assert_eq!(state.sessions[0].id, "session-12345678");
        assert_eq!(state.sessions[0].model, "gpt-new");
        assert!(store
            .load(&key)
            .sessions
            .iter()
            .any(|session| session.model == "gpt-new"));
    }

    #[test]
    fn resume_finds_saved_session_and_reports_missing_targets() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-default".to_string());
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: Some(99),
        };

        assert!(store
            .save_current_session(&key, 0, "019e778b-2c3f-7231-bda6-c40f27bbba21")
            .unwrap());
        let renamed = store.rename_current(&key, "daily").unwrap();
        assert_eq!(renamed.sessions[0].name.as_deref(), Some("daily"));

        let resumed = store.resume(&key, "daily").unwrap();
        assert_eq!(
            resumed.session_id.as_deref(),
            Some("019e778b-2c3f-7231-bda6-c40f27bbba21")
        );
        assert_eq!(resumed.generation, 1);
        assert!(store
            .resume(&key, "missing")
            .unwrap_err()
            .contains("No saved session"));
    }

    #[test]
    fn rename_current_requires_current_session() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-default".to_string());
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: None,
        };

        let err = store.rename_current(&key, "name").unwrap_err();

        assert!(err.contains("No current session"));
    }

    #[test]
    fn list_formats_current_and_default_model_sessions() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-default".to_string());
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: Some(99),
        };

        assert_eq!(
            store.list(&key),
            "📭 No saved sessions yet. Send a normal message to create one."
        );
        assert!(store
            .save_current_session(&key, 0, "session-current")
            .unwrap());
        let mut state = store.load(&key);
        state.sessions.push(SavedSession {
            id: "session-older".to_string(),
            name: None,
            model: String::new(),
            provider: Provider::Codex,
            updated_at: String::new(),
        });
        store.save(&key, &state).unwrap();

        let list = store.list(&key);

        assert!(list.contains("💾 Saved sessions:"));
        assert!(list.contains("⭐ Codex session- gpt-default (unnamed)"));
        assert!(list.contains("▫️ Codex session- gpt-default (unnamed)"));
        assert!(dir.path().join("chats/7-thread-99.json").exists());
    }

    #[test]
    fn load_keeps_missing_session_list_empty() {
        let dir = tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("chats"), "gpt-default".to_string());
        let path = dir.path().join("chats/7-main.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"session_id":"old-session","model":" "}"#).unwrap();
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: None,
        };

        let state = store.load(&key);

        assert_eq!(state.model, "gpt-default");
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn upsert_ignores_empty_ids_and_fills_default_model() {
        let existing = vec![SavedSession {
            id: "existing".to_string(),
            name: None,
            model: "gpt-old".to_string(),
            provider: Provider::Codex,
            updated_at: String::new(),
        }];

        let unchanged = upsert_session(
            existing.clone(),
            SavedSession {
                id: " ".to_string(),
                name: None,
                model: String::new(),
                provider: Provider::Codex,
                updated_at: String::new(),
            },
            "gpt-default",
        );
        let inserted = upsert_session(
            existing,
            SavedSession {
                id: " new ".to_string(),
                name: Some("named".to_string()),
                model: " ".to_string(),
                provider: Provider::Codex,
                updated_at: String::new(),
            },
            "gpt-default",
        );

        assert_eq!(unchanged.len(), 1);
        assert_eq!(inserted[0].id, "new");
        assert_eq!(inserted[0].model, "gpt-default");
        assert_eq!(inserted.len(), 2);
    }

    #[test]
    fn save_reports_session_directory_creation_errors() {
        let dir = tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "file").unwrap();
        let store = SessionStore::new(blocked.join("chats"), "gpt-default".to_string());
        let key = SessionKey::Chat {
            chat_id: 7,
            thread_id: None,
        };

        let err = store.reset(&key).unwrap_err();

        assert!(err.contains("create session dir"));
    }
}
