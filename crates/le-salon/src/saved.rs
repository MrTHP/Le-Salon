//! Messages sauvegardés localement avec note et libellé.
//!
//! Contrairement à l'historique complet, ces instantanés ne sont pas effacés
//! par `/clear` et restent consultables depuis la fenêtre « Messages
//! sauvegardés ».

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use ircv3_core::model::BufferKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedMessage {
    pub id: u64,
    pub network: String,
    pub kind: BufferKind,
    pub buffer: String,
    pub nick: Option<String>,
    pub text: String,
    pub at: String,
    pub msgid: Option<String>,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub label: String,
    pub saved_at: String,
}

impl SavedMessage {
    pub fn at_label(&self) -> String {
        DateTime::parse_from_rfc3339(&self.at)
            .map(|at| at.with_timezone(&Local).format("%d/%m %H:%M").to_string())
            .unwrap_or_else(|_| self.at.clone())
    }

    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        self.text.to_lowercase().contains(&query)
            || self.buffer.to_lowercase().contains(&query)
            || self
                .nick
                .as_deref()
                .is_some_and(|nick| nick.to_lowercase().contains(&query))
            || self.note.to_lowercase().contains(&query)
            || self.label.to_lowercase().contains(&query)
    }
}

pub struct SavedStore {
    pub items: Vec<SavedMessage>,
    next_id: u64,
    path: PathBuf,
}

impl SavedStore {
    pub fn load() -> Self {
        let path = ircv3_core::Config::path()
            .parent()
            .map(|parent| parent.join("saved-messages.json"))
            .unwrap_or_else(|| PathBuf::from("saved-messages.json"));
        let items = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<SavedMessage>>(&text).ok())
            .unwrap_or_default();
        let next_id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        Self {
            items,
            next_id,
            path,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        network: String,
        kind: BufferKind,
        buffer: String,
        nick: Option<String>,
        text: String,
        at: DateTime<Local>,
        msgid: Option<String>,
    ) -> u64 {
        // Évite les doublons quand l'utilisateur sauvegarde deux fois.
        if let Some(existing) = self.items.iter_mut().find(|item| {
            item.network == network
                && item.buffer == buffer
                && item.nick == nick
                && item.text == text
                && item.at == at.to_rfc3339()
        }) {
            return existing.id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(SavedMessage {
            id,
            network,
            kind,
            buffer,
            nick,
            text,
            at: at.to_rfc3339(),
            msgid,
            note: String::new(),
            label: String::new(),
            saved_at: Local::now().to_rfc3339(),
        });
        self.save();
        id
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        let changed = self.items.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    pub fn set_note(&mut self, id: u64, note: String, label: String) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        item.note = note;
        item.label = label;
        self.save();
        true
    }

    pub fn filtered(&self, query: &str) -> Vec<&SavedMessage> {
        self.items
            .iter()
            .filter(|item| item.matches(query))
            .collect()
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
            restrict_dir(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.items)
            && let Err(error) = write_private(&self.path, &text)
        {
            log::warn!("sauvegarde des messages sauvegardés impossible : {error}");
        }
    }
}

fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    let _ = path;
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, text)
    }
}
