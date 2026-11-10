//! Historique local des conversations.
//!
//! Chaque tampon est stocké dans un fichier JSONL sous
//! `~/.config/le-salon/history/`. Les lignes brutes sont conservées afin de
//! pouvoir restaurer le formatage, les identifiants de message, les réponses
//! et les réactions après un redémarrage.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    model::{BufferKind, ChatLine, LineKind, MAX_MESSAGES},
};

/// Taille maximale approximative d'un fichier avant compactage.
const MAX_HISTORY_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct HistoryStore {
    root: PathBuf,
}

#[derive(Clone, Debug)]
pub struct StoredBuffer {
    pub kind: BufferKind,
    pub name: String,
    pub lines: Vec<ChatLine>,
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub network: String,
    pub kind: BufferKind,
    pub name: String,
    pub line: ChatLine,
}

#[derive(Serialize, Deserialize)]
struct StoredLine {
    at: String,
    kind: LineKind,
    nick: Option<String>,
    raw: String,
    #[serde(default)]
    self_nick: bool,
    #[serde(default)]
    mention: bool,
    #[serde(default)]
    msgid: Option<String>,
    #[serde(default)]
    reply_to: Option<String>,
    #[serde(default)]
    reactions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    buffer_kind: BufferKind,
    #[serde(default)]
    buffer_name: String,
    #[serde(default)]
    network: String,
}

impl StoredLine {
    fn from_line(network: &str, kind: BufferKind, name: &str, line: &ChatLine) -> Self {
        Self {
            at: line.at.to_rfc3339(),
            kind: line.kind,
            nick: line.nick.clone(),
            raw: line.raw.clone(),
            self_nick: line.self_nick,
            mention: line.mention,
            msgid: line.msgid.clone(),
            reply_to: line.reply_to.clone(),
            reactions: line.reactions.clone(),
            buffer_kind: kind,
            buffer_name: name.to_string(),
            network: network.to_string(),
        }
    }

    fn into_line(self) -> ChatLine {
        let at = DateTime::parse_from_rfc3339(&self.at)
            .map(|value| value.with_timezone(&Local))
            .unwrap_or_else(|_| Local::now());
        let mut line = ChatLine::new(self.kind, self.nick, self.raw).with_time(at);
        line.self_nick = self.self_nick;
        line.mention = self.mention;
        line.msgid = self.msgid;
        line.reply_to = self.reply_to;
        line.reactions = self.reactions;
        line
    }
}

impl Default for HistoryStore {
    fn default() -> Self {
        let root = Config::path()
            .parent()
            .map(|path| path.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".config/le-salon"))
            .join("history");
        Self::new(root)
    }
}

impl HistoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn network_dir(&self, network: &str) -> PathBuf {
        self.root.join(sanitize(network))
    }

    fn buffer_path(&self, network: &str, kind: BufferKind, name: &str) -> PathBuf {
        self.network_dir(network).join(format!(
            "{}-{}-{:016x}.jsonl",
            kind_label(kind),
            sanitize(name),
            stable_hash(name)
        ))
    }

    /// Charge les derniers messages d'un tampon.
    pub fn load_buffer(&self, network: &str, kind: BufferKind, name: &str) -> Vec<ChatLine> {
        let path = self.buffer_path(network, kind, name);
        let Ok(file) = File::open(&path) else {
            return Vec::new();
        };
        let mut raw_lines: VecDeque<String> = VecDeque::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            raw_lines.push_back(line);
            if raw_lines.len() > MAX_MESSAGES {
                raw_lines.pop_front();
            }
        }
        raw_lines
            .into_iter()
            .filter_map(|line| serde_json::from_str::<StoredLine>(&line).ok())
            .map(StoredLine::into_line)
            .collect()
    }

    /// Charge tous les tampons connus d'un réseau, pour l'affichage hors ligne
    /// et la recherche locale.
    pub fn load_network(&self, network: &str) -> Vec<StoredBuffer> {
        let dir = self.network_dir(network);
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut buffers: BTreeMap<(String, String), StoredBuffer> = BTreeMap::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(file) = File::open(&path) else {
                continue;
            };
            let mut lines: VecDeque<ChatLine> = VecDeque::new();
            let mut buffer_kind = BufferKind::Server;
            let mut buffer_name = String::new();
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                let Ok(stored) = serde_json::from_str::<StoredLine>(&line) else {
                    continue;
                };
                if buffer_name.is_empty() {
                    buffer_name = stored.buffer_name.clone();
                    buffer_kind = stored.buffer_kind;
                }
                lines.push_back(stored.into_line());
                if lines.len() > MAX_MESSAGES {
                    lines.pop_front();
                }
            }
            if buffer_name.is_empty() {
                continue;
            }
            let key = (buffer_kind_label(buffer_kind), buffer_name.clone());
            buffers.insert(
                key,
                StoredBuffer {
                    kind: buffer_kind,
                    name: buffer_name,
                    lines: lines.into_iter().collect(),
                },
            );
        }
        buffers.into_values().collect()
    }

    /// Ajoute une ligne à l'historique du tampon.
    pub fn append(
        &self,
        network: &str,
        kind: BufferKind,
        name: &str,
        line: &ChatLine,
    ) -> io::Result<()> {
        let path = self.buffer_path(network, kind, name);
        append_json_line(&path, &StoredLine::from_line(network, kind, name, line))?;
        compact_if_needed(&path)
    }

    /// Recherche insensible à la casse dans tous les messages stockés.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut hits = Vec::new();
        for entry in entries.flatten() {
            let network_dir = entry.path();
            if !network_dir.is_dir() {
                continue;
            }
            let Ok(files) = fs::read_dir(&network_dir) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                let Ok(handle) = File::open(&path) else {
                    continue;
                };
                for line in BufReader::new(handle).lines().map_while(Result::ok) {
                    let Ok(stored) = serde_json::from_str::<StoredLine>(&line) else {
                        continue;
                    };
                    if !stored.raw.to_lowercase().contains(&needle) {
                        continue;
                    }
                    hits.push(SearchHit {
                        network: stored.network.clone(),
                        kind: stored.buffer_kind,
                        name: stored.buffer_name.clone(),
                        line: stored.into_line(),
                    });
                    if hits.len() >= limit.saturating_mul(4) {
                        break;
                    }
                }
            }
        }
        hits.sort_by(|left, right| right.line.at.cmp(&left.line.at));
        hits.truncate(limit);
        hits
    }

    /// Réécrit intégralement un tampon (mise à jour des réactions).
    pub fn rewrite(
        &self,
        network: &str,
        kind: BufferKind,
        name: &str,
        lines: &[ChatLine],
    ) -> io::Result<()> {
        let path = self.buffer_path(network, kind, name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            set_private_dir(parent);
        }
        let temp = path.with_extension("jsonl.tmp");
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        for line in lines {
            let stored = StoredLine::from_line(network, kind, name, line);
            serde_json::to_writer(&mut file, &stored).map_err(io::Error::other)?;
            file.write_all(b"\n")?;
        }
        file.flush()?;
        fs::rename(&temp, &path)?;
        Ok(())
    }

    /// Supprime l'historique d'un tampon (commande `/clear`).
    pub fn clear_buffer(&self, network: &str, kind: BufferKind, name: &str) -> io::Result<()> {
        let path = self.buffer_path(network, kind, name);
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Nombre total de messages stockés, pour l'affichage éventuel.
    pub fn message_count(&self) -> usize {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return 0;
        };
        let mut count = 0;
        for entry in entries.flatten() {
            let Ok(files) = fs::read_dir(entry.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(handle) = File::open(path) {
                    count += BufReader::new(handle)
                        .lines()
                        .map_while(Result::ok)
                        .filter(|line| !line.trim().is_empty())
                        .count();
                }
            }
        }
        count
    }
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_private_dir(parent);
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn compact_if_needed(path: &Path) -> io::Result<()> {
    let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size <= MAX_HISTORY_BYTES {
        return Ok(());
    }
    let file = File::open(path)?;
    let mut lines: VecDeque<String> = VecDeque::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        lines.push_back(line);
        if lines.len() > MAX_MESSAGES {
            lines.pop_front();
        }
    }
    let temp = path.with_extension("jsonl.tmp");
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    for line in lines {
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }
    file.flush()?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn set_private_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    let _ = path;
}

fn kind_label(kind: BufferKind) -> &'static str {
    match kind {
        BufferKind::Server => "server",
        BufferKind::Channel => "channel",
        BufferKind::Query => "query",
    }
}

fn buffer_kind_label(kind: BufferKind) -> String {
    kind_label(kind).to_string()
}

fn sanitize(value: &str) -> String {
    let mut out = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            out.push(character);
        } else {
            out.push('_');
        }
        if out.len() >= 96 {
            break;
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[allow(dead_code)]
fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("le-salon-history-{label}-{stamp}"))
    }

    #[test]
    fn roundtrip_and_search() {
        let root = temporary_root("roundtrip");
        let store = HistoryStore::new(&root);
        let mut line = ChatLine::new(
            LineKind::Message,
            Some("alice".into()),
            "bonjour \u{2}monde\u{2}",
        );
        line.msgid = Some("msg-1".into());
        line.reply_to = Some("parent".into());
        line.add_reaction("👍", "bob");
        store
            .append("irc.test:6697", BufferKind::Query, "alice", &line)
            .unwrap();

        let loaded = store.load_buffer("irc.test:6697", BufferKind::Query, "alice");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].text, "bonjour monde");
        assert_eq!(loaded[0].msgid.as_deref(), Some("msg-1"));
        assert_eq!(loaded[0].reply_to.as_deref(), Some("parent"));
        assert_eq!(loaded[0].reactions["👍"], vec!["bob"]);

        let hits = store.search("bonjour", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "alice");

        let network = store.load_network("irc.test:6697");
        assert_eq!(network.len(), 1);
        assert_eq!(network[0].name, "alice");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_is_case_insensitive() {
        let root = temporary_root("search");
        let store = HistoryStore::new(&root);
        let line = ChatLine::new(LineKind::Message, Some("bob".into()), "Salut ALICE");
        store
            .append("irc.test:6697", BufferKind::Channel, "#test", &line)
            .unwrap();
        assert_eq!(store.search("alice", 10).len(), 1);
        assert!(store.search("introuvable", 10).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
