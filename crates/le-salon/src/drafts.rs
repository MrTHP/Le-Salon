//! Brouillons récupérables après redémarrage.
//!
//! Les brouillons sont stockés localement dans `drafts.json`, avec une
//! expiration de sept jours comme les brouillons de navigateur de Relay.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};

const DRAFT_MAX_AGE_DAYS: i64 = 7;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DraftEntry {
    text: String,
    updated: String,
}

pub struct DraftStore {
    entries: BTreeMap<String, DraftEntry>,
    path: PathBuf,
    dirty: bool,
}

impl DraftStore {
    pub fn load() -> Self {
        let path = ircv3_core::Config::path()
            .parent()
            .map(|parent| parent.join("drafts.json"))
            .unwrap_or_else(|| PathBuf::from("drafts.json"));
        let entries = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<BTreeMap<String, DraftEntry>>(&text).ok())
            .unwrap_or_default();
        let mut store = Self {
            entries,
            path,
            dirty: false,
        };
        store.prune_expired();
        store
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).map(|entry| entry.text.clone())
    }

    pub fn set(&mut self, key: &str, text: &str) {
        if text.trim().is_empty() {
            self.remove(key);
            return;
        }
        let changed = self.entries.get(key).is_none_or(|entry| entry.text != text);
        if changed {
            self.entries.insert(
                key.to_string(),
                DraftEntry {
                    text: text.to_string(),
                    updated: Local::now().to_rfc3339(),
                },
            );
            self.dirty = true;
        }
    }

    pub fn remove(&mut self, key: &str) {
        if self.entries.remove(key).is_some() {
            self.dirty = true;
        }
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
            restrict_dir(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.entries)
            && let Err(error) = write_private(&self.path, &text)
        {
            log::warn!("sauvegarde des brouillons impossible : {error}");
            return;
        }
        self.dirty = false;
    }

    fn prune_expired(&mut self) {
        let cutoff = Local::now() - Duration::days(DRAFT_MAX_AGE_DAYS);
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            DateTime::parse_from_rfc3339(&entry.updated)
                .map(|updated| updated.with_timezone(&Local) >= cutoff)
                .unwrap_or(false)
        });
        if self.entries.len() != before {
            self.dirty = true;
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
