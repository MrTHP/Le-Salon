//! Modèle de session partagé entre le thread réseau et l'interface GPUI.
//!
//! Toutes les mutations passent par un `Arc<Mutex<AppModel>>` ; les messages
//! sont conservés par tampon (serveur, salon, privé) avec un plafond pour
//! éviter une croissance illimitée.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::{
    config::{Config, ConnectionConfig},
    formatting::{self, FormatSpan},
    history::HistoryStore,
    irc_fold,
};

pub type NetworkId = u64;
pub type BufferId = u64;

pub const MAX_MESSAGES: usize = 2_000;
pub const TYPING_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Registering,
    Connected,
}

impl ConnectionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => crate::i18n::t("hors ligne", "offline"),
            Self::Connecting => crate::i18n::t("connexion…", "connecting…"),
            Self::Registering => crate::i18n::t("négociation…", "negotiating…"),
            Self::Connected => crate::i18n::t("connecté", "connected"),
        }
    }

    pub fn is_online(self) -> bool {
        matches!(self, Self::Connected | Self::Registering)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferKind {
    #[default]
    Server,
    Channel,
    Query,
}

impl BufferKind {
    pub fn icon(self) -> &'static str {
        match self {
            Self::Server => "•",
            Self::Channel => "#",
            Self::Query => "@",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    #[default]
    Message,
    Action,
    Notice,
    Join,
    Part,
    Quit,
    Nick,
    Mode,
    Topic,
    Kick,
    Invite,
    Away,
    Account,
    Server,
    Motd,
    Error,
    Info,
    History,
    Other,
}

#[derive(Clone, Debug)]
pub struct ChatLine {
    pub at: DateTime<Local>,
    pub kind: LineKind,
    pub nick: Option<String>,
    /// Texte brut tel qu'il a été reçu ou envoyé, avant analyse du formatage.
    pub raw: String,
    pub text: String,
    pub spans: Vec<FormatSpan>,
    pub self_nick: bool,
    pub mention: bool,
    /// Identifiant IRCv3 (`msgid`) permettant les réponses et réactions.
    pub msgid: Option<String>,
    /// Identifiant du message auquel celui-ci répond (`+draft/reply`).
    pub reply_to: Option<String>,
    /// Réactions : emoji → pseudos.
    pub reactions: BTreeMap<String, Vec<String>>,
}

impl ChatLine {
    pub fn new(kind: LineKind, nick: Option<String>, text: impl Into<String>) -> Self {
        let raw = text.into();
        let formatted = formatting::parse(&raw);
        Self {
            at: Local::now(),
            kind,
            nick,
            raw,
            text: formatted.text,
            spans: formatted.spans,
            self_nick: false,
            mention: false,
            msgid: None,
            reply_to: None,
            reactions: BTreeMap::new(),
        }
    }

    pub fn with_time(mut self, at: DateTime<Local>) -> Self {
        self.at = at;
        self
    }

    pub fn with_msgid(mut self, msgid: Option<String>) -> Self {
        self.msgid = msgid;
        self
    }

    pub fn with_reply_to(mut self, reply_to: Option<String>) -> Self {
        self.reply_to = reply_to;
        self
    }

    pub fn add_reaction(&mut self, emoji: &str, nick: &str) {
        let entry = self.reactions.entry(emoji.to_string()).or_default();
        if !entry
            .iter()
            .any(|existing| irc_fold(existing) == irc_fold(nick))
        {
            entry.push(nick.to_string());
            entry.sort_by_key(|value| irc_fold(value));
        }
    }

    pub fn remove_reaction(&mut self, emoji: &str, nick: &str) {
        let Some(entry) = self.reactions.get_mut(emoji) else {
            return;
        };
        entry.retain(|existing| irc_fold(existing) != irc_fold(nick));
        if entry.is_empty() {
            self.reactions.remove(emoji);
        }
    }

    pub fn is_plain(&self) -> bool {
        self.spans.iter().all(FormatSpan::is_plain)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct User {
    pub nick: String,
    pub prefixes: String,
    pub user: Option<String>,
    pub host: Option<String>,
    pub account: Option<String>,
    pub realname: Option<String>,
    pub away: bool,
}

impl User {
    pub fn new(nick: impl Into<String>) -> Self {
        Self {
            nick: nick.into(),
            ..Default::default()
        }
    }

    /// Rang de tri : owner, admin, op, halfop, voice, simple.
    pub fn rank(&self) -> u8 {
        self.prefixes
            .chars()
            .map(|c| match c {
                '~' => 0,
                '&' => 1,
                '@' => 2,
                '%' => 3,
                '+' => 4,
                _ => 5,
            })
            .min()
            .unwrap_or(5)
    }

    pub fn best_prefix(&self) -> Option<char> {
        self.prefixes.chars().min_by_key(|c| match c {
            '~' => 0,
            '&' => 1,
            '@' => 2,
            '%' => 3,
            '+' => 4,
            _ => 5,
        })
    }

    pub fn add_prefix(&mut self, prefix: char) {
        if !self.prefixes.contains(prefix) {
            self.prefixes.push(prefix);
            self.prefixes = self
                .prefixes
                .chars()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
        }
    }

    pub fn remove_prefix(&mut self, prefix: char) {
        self.prefixes.retain(|c| c != prefix);
    }

    pub fn hostmask(&self) -> String {
        match (&self.user, &self.host) {
            (Some(user), Some(host)) => format!("{}!{}@{}", self.nick, user, host),
            (Some(user), None) => format!("{}!{}", self.nick, user),
            _ => self.nick.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Buffer {
    pub id: BufferId,
    pub network: NetworkId,
    pub kind: BufferKind,
    pub name: String,
    pub topic: String,
    pub topic_by: Option<String>,
    pub topic_at: Option<DateTime<Local>>,
    pub modes: String,
    pub users: Vec<User>,
    pub messages: VecDeque<ChatLine>,
    pub unread: u32,
    pub highlight: bool,
    pub joined: bool,
    pub typing: HashMap<String, Instant>,
    pub created_at: Option<DateTime<Local>>,
    /// Incrémenté à chaque nouveau message ; l'interface s'en sert pour
    /// décider quand replacer la vue en bas.
    pub version: u64,
    pub last_activity: DateTime<Local>,
}

impl Buffer {
    pub fn new(
        id: BufferId,
        network: NetworkId,
        kind: BufferKind,
        name: impl Into<String>,
    ) -> Self {
        let name = name.into();
        Self {
            id,
            network,
            kind,
            name,
            topic: String::new(),
            topic_by: None,
            topic_at: None,
            modes: String::new(),
            users: Vec::new(),
            messages: VecDeque::new(),
            unread: 0,
            highlight: false,
            joined: matches!(kind, BufferKind::Server | BufferKind::Query),
            typing: HashMap::new(),
            created_at: None,
            version: 0,
            last_activity: Local::now(),
        }
    }

    pub fn is_channel(&self) -> bool {
        self.kind == BufferKind::Channel
    }

    pub fn is_query(&self) -> bool {
        self.kind == BufferKind::Query
    }

    pub fn add_message(&mut self, line: ChatLine) {
        self.last_activity = line.at;
        self.messages.push_back(line);
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.version = self.version.wrapping_add(1);
    }

    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.unread = 0;
        self.highlight = false;
        self.version = self.version.wrapping_add(1);
    }

    pub fn user(&self, nick: &str) -> Option<&User> {
        let folded = irc_fold(nick);
        self.users
            .iter()
            .find(|user| irc_fold(&user.nick) == folded)
    }

    pub fn user_mut(&mut self, nick: &str) -> Option<&mut User> {
        let folded = irc_fold(nick);
        self.users
            .iter_mut()
            .find(|user| irc_fold(&user.nick) == folded)
    }

    pub fn add_or_update_user(&mut self, user: User) {
        if let Some(existing) = self.user_mut(&user.nick) {
            for prefix in user.prefixes.chars() {
                existing.add_prefix(prefix);
            }
            if user.user.is_some() {
                existing.user = user.user;
            }
            if user.host.is_some() {
                existing.host = user.host;
            }
            if user.account.is_some() {
                existing.account = user.account;
            }
            if user.realname.is_some() {
                existing.realname = user.realname;
            }
            existing.away = user.away;
            return;
        }
        self.users.push(user);
        self.sort_users();
    }

    pub fn remove_user(&mut self, nick: &str) -> Option<User> {
        let folded = irc_fold(nick);
        let index = self
            .users
            .iter()
            .position(|user| irc_fold(&user.nick) == folded)?;
        Some(self.users.remove(index))
    }

    pub fn rename_user(&mut self, old: &str, new: &str) -> Option<String> {
        let user = self.user_mut(old)?;
        let previous = user.nick.clone();
        user.nick = new.to_string();
        self.sort_users();
        Some(previous)
    }

    pub fn sort_users(&mut self) {
        self.users.sort_by(|a, b| {
            a.rank()
                .cmp(&b.rank())
                .then_with(|| irc_fold(&a.nick).cmp(&irc_fold(&b.nick)))
        });
    }

    pub fn users_sorted(&self) -> Vec<User> {
        let mut users = self.users.clone();
        users.sort_by(|a, b| {
            a.rank()
                .cmp(&b.rank())
                .then_with(|| irc_fold(&a.nick).cmp(&irc_fold(&b.nick)))
        });
        users
    }

    pub fn set_typing(&mut self, nick: &str) {
        self.typing.insert(irc_fold(nick), Instant::now());
    }

    pub fn typing_users(&self) -> Vec<String> {
        let now = Instant::now();
        let mut users: Vec<_> = self
            .typing
            .iter()
            .filter(|(_, at)| now.duration_since(**at) < TYPING_TIMEOUT)
            .map(|(nick, _)| nick.clone())
            .collect();
        users.sort();
        users
    }

    pub fn prune_typing(&mut self) {
        let now = Instant::now();
        self.typing
            .retain(|_, at| now.duration_since(*at) < TYPING_TIMEOUT);
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    pub fn unread_label(&self) -> Option<String> {
        if self.unread == 0 {
            None
        } else if self.unread > 99 {
            Some("99+".into())
        } else {
            Some(self.unread.to_string())
        }
    }

    pub fn title(&self) -> String {
        match self.kind {
            BufferKind::Server => format!("{} — serveur", self.name),
            _ => self.name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DccStatus {
    Waiting,
    Connected,
    Sending,
    Done,
    Failed,
    Canceled,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelListEntry {
    pub channel: String,
    pub users: usize,
    pub topic: String,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelListState {
    pub entries: Vec<ChannelListEntry>,
    pub loading: bool,
    pub complete: bool,
    pub updated_at: Option<DateTime<Local>>,
}

#[derive(Clone, Debug)]
pub struct DccTransfer {
    pub id: u64,
    pub network: NetworkId,
    pub nick: String,
    pub filename: String,
    pub path: PathBuf,
    pub size: u64,
    pub sent: u64,
    pub status: DccStatus,
    pub error: Option<String>,
    pub started_at: DateTime<Local>,
    pub cancel: Arc<AtomicBool>,
}

impl DccTransfer {
    pub fn percent(&self) -> f32 {
        if self.size == 0 {
            100.0
        } else {
            (self.sent as f32 / self.size as f32 * 100.0).clamp(0.0, 100.0)
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            DccStatus::Waiting | DccStatus::Connected | DccStatus::Sending
        )
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct Network {
    pub id: NetworkId,
    /// Nom du réseau (005 NETWORK) ou, à défaut, l'hôte.
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_tls: bool,
    pub nick: String,
    pub user: String,
    pub realname: String,
    pub status: ConnectionStatus,
    pub status_text: String,
    pub caps: Vec<String>,
    pub buffers: Vec<BufferId>,
    pub server_buffer: BufferId,
    pub lag_ms: Option<u64>,
    pub connected_at: Option<DateTime<Local>>,
    pub last_error: Option<String>,
    pub prefix_modes: Vec<(char, char)>,
    pub chantypes: String,
    pub casemapping: String,
    pub away: bool,
    pub monitor: Vec<String>,
    pub config: ConnectionConfig,
}

impl Network {
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.host
        } else {
            &self.name
        }
    }

    pub fn endpoint(&self) -> String {
        if self.tls {
            format!("{}:{} (TLS)", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn prefix_for_mode(&self, mode: char) -> Option<char> {
        self.prefix_modes
            .iter()
            .find_map(|(m, p)| (*m == mode).then_some(*p))
    }

    pub fn mode_for_prefix(&self, prefix: char) -> Option<char> {
        self.prefix_modes
            .iter()
            .find_map(|(m, p)| (*p == prefix).then_some(*m))
    }

    pub fn channel_count(&self) -> usize {
        self.buffers.len().saturating_sub(
            self.buffers
                .iter()
                .filter(|id| id == &&self.server_buffer)
                .count(),
        )
    }
}

pub struct AppModel {
    pub networks: Vec<Network>,
    pub buffers: BTreeMap<BufferId, Buffer>,
    pub active: Option<BufferId>,
    pub config: Config,
    pub next_id: u64,
    pub next_dcc_id: u64,
    pub dcc: Vec<DccTransfer>,
    pub channel_lists: HashMap<NetworkId, ChannelListState>,
    pub version: u64,
    /// Stockage local de l'historique. Désactivé dans les tests et les
    /// utilisations embarquées qui ne doivent rien écrire sur le disque.
    pub history: Option<HistoryStore>,
}

impl AppModel {
    pub fn new(config: Config) -> Self {
        Self {
            networks: Vec::new(),
            buffers: BTreeMap::new(),
            active: None,
            config,
            next_id: 1,
            next_dcc_id: 1,
            dcc: Vec::new(),
            channel_lists: HashMap::new(),
            version: 0,
            history: None,
        }
    }

    pub fn enable_history(&mut self, history: HistoryStore) {
        self.history = Some(history);
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    pub fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub fn add_network(&mut self, config: ConnectionConfig) -> NetworkId {
        let network_id = self.alloc_id();
        let server_id = self.alloc_id();
        let name = if config.host.is_empty() {
            "Nouveau réseau".to_string()
        } else {
            config.host.clone()
        };
        let mut server = Buffer::new(server_id, network_id, BufferKind::Server, name.clone());
        server.joined = true;
        self.buffers.insert(server_id, server);
        let network = Network {
            id: network_id,
            name,
            host: config.host.clone(),
            port: config.port,
            tls: config.tls,
            verify_tls: config.verify_tls,
            nick: config.nick.clone(),
            user: config.user.clone(),
            realname: config.realname.clone(),
            status: ConnectionStatus::Connecting,
            status_text: "connexion…".into(),
            caps: Vec::new(),
            buffers: vec![server_id],
            server_buffer: server_id,
            lag_ms: None,
            connected_at: None,
            last_error: None,
            prefix_modes: vec![('o', '@'), ('v', '+')],
            chantypes: "#&+!".into(),
            casemapping: "rfc1459".into(),
            away: false,
            monitor: Vec::new(),
            config,
        };
        self.networks.push(network);
        self.active = Some(server_id);
        self.touch();
        network_id
    }

    pub fn remove_network(&mut self, network_id: NetworkId) {
        self.channel_lists.remove(&network_id);
        if let Some(network) = self.networks.iter().find(|n| n.id == network_id) {
            let ids = network.buffers.clone();
            for id in ids {
                self.buffers.remove(&id);
            }
        }
        self.networks.retain(|n| n.id != network_id);
        if self
            .active
            .is_some_and(|id| !self.buffers.contains_key(&id))
        {
            self.active = self.buffers.keys().next().copied();
        }
        self.touch();
    }

    pub fn network(&self, id: NetworkId) -> Option<&Network> {
        self.networks.iter().find(|network| network.id == id)
    }

    pub fn network_mut(&mut self, id: NetworkId) -> Option<&mut Network> {
        self.networks.iter_mut().find(|network| network.id == id)
    }

    pub fn network_history_key(&self, network_id: NetworkId) -> Option<String> {
        self.network(network_id)
            .map(|network| format!("{}:{}", network.host, network.port))
    }

    pub fn buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.buffers.get(&id)
    }

    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut Buffer> {
        self.buffers.get_mut(&id)
    }

    pub fn active_buffer(&self) -> Option<&Buffer> {
        self.active.and_then(|id| self.buffers.get(&id))
    }

    pub fn active_network(&self) -> Option<&Network> {
        self.active_buffer()
            .and_then(|buffer| self.network(buffer.network))
    }

    pub fn active_network_mut(&mut self) -> Option<&mut Network> {
        let id = self.active_buffer()?.network;
        self.network_mut(id)
    }

    pub fn find_buffer(
        &self,
        network_id: NetworkId,
        kind: BufferKind,
        name: &str,
    ) -> Option<BufferId> {
        let folded = irc_fold(name);
        self.buffers.values().find_map(|buffer| {
            (buffer.network == network_id
                && buffer.kind == kind
                && irc_fold(&buffer.name) == folded)
                .then_some(buffer.id)
        })
    }

    pub fn ensure_buffer(
        &mut self,
        network_id: NetworkId,
        kind: BufferKind,
        name: &str,
    ) -> BufferId {
        if let Some(id) = self.find_buffer(network_id, kind, name) {
            return id;
        }
        let id = self.alloc_id();
        let mut buffer = Buffer::new(id, network_id, kind, name);
        buffer.joined = matches!(kind, BufferKind::Server | BufferKind::Query);
        if kind != BufferKind::Server
            && let (Some(history), Some(network)) = (&self.history, self.network(network_id))
        {
            let key = format!("{}:{}", network.host, network.port);
            let mut lines = history.load_buffer(&key, kind, name);
            if !lines.is_empty() {
                buffer.messages.extend(lines.drain(..));
                buffer.version = buffer.version.wrapping_add(1);
                if let Some(last) = buffer.messages.back() {
                    buffer.last_activity = last.at;
                }
            }
        }
        self.buffers.insert(id, buffer);
        if let Some(network) = self.network_mut(network_id) {
            network.buffers.push(id);
        }
        if self.active.is_none() {
            self.active = Some(id);
        }
        self.touch();
        id
    }

    pub fn server_buffer(&self, network_id: NetworkId) -> Option<BufferId> {
        self.network(network_id)
            .map(|network| network.server_buffer)
    }

    pub fn set_active(&mut self, buffer_id: BufferId) {
        if self.buffers.contains_key(&buffer_id) {
            self.active = Some(buffer_id);
            if let Some(buffer) = self.buffer_mut(buffer_id) {
                buffer.unread = 0;
                buffer.highlight = false;
            }
            self.touch();
        }
    }

    pub fn append_line(&mut self, buffer_id: BufferId, line: ChatLine) {
        let active = self.active == Some(buffer_id);
        let mention = line.mention && !line.self_nick;
        let persist = self.history.as_ref().and_then(|history| {
            let buffer = self.buffers.get(&buffer_id)?;
            if buffer.kind == BufferKind::Server {
                return None;
            }
            let network = self.network(buffer.network)?;
            Some(history.append(
                &format!("{}:{}", network.host, network.port),
                buffer.kind,
                &buffer.name,
                &line,
            ))
        });
        if let Some(Err(error)) = persist {
            log::warn!("écriture de l'historique impossible : {error}");
        }
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            if !active && buffer.kind != BufferKind::Server {
                buffer.unread = buffer.unread.saturating_add(1);
                buffer.highlight |= mention;
            }
            buffer.add_message(line);
            self.touch();
        }
    }

    pub fn apply_reaction(
        &mut self,
        buffer_id: BufferId,
        msgid: &str,
        emoji: &str,
        nick: &str,
        add: bool,
    ) -> bool {
        let mut updated = false;
        let mut snapshot = None;
        if let Some(buffer) = self.buffers.get_mut(&buffer_id) {
            if let Some(line) = buffer
                .messages
                .iter_mut()
                .rev()
                .find(|line| line.msgid.as_deref() == Some(msgid))
            {
                if add {
                    line.add_reaction(emoji, nick);
                } else {
                    line.remove_reaction(emoji, nick);
                }
                updated = true;
            }
            if updated {
                snapshot = Some((
                    buffer.kind,
                    buffer.name.clone(),
                    buffer.messages.iter().cloned().collect::<Vec<_>>(),
                ));
            }
        }
        if updated
            && let Some((kind, name, lines)) = snapshot
            && let Some(history) = &self.history
            && let Some(network) = self
                .buffers
                .get(&buffer_id)
                .and_then(|buffer| self.network(buffer.network))
            && let Err(error) = history.rewrite(
                &format!("{}:{}", network.host, network.port),
                kind,
                &name,
                &lines,
            )
        {
            log::warn!("sauvegarde des réactions impossible : {error}");
        }
        if updated {
            self.touch();
        }
        updated
    }

    pub fn clear_buffer(&mut self, buffer_id: BufferId) {
        if let Some(history) = &self.history
            && let Some(buffer) = self.buffers.get(&buffer_id)
            && let Some(network) = self.network(buffer.network)
            && let Err(error) = history.clear_buffer(
                &format!("{}:{}", network.host, network.port),
                buffer.kind,
                &buffer.name,
            )
        {
            log::warn!("suppression de l'historique impossible : {error}");
        }
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.clear_messages();
            self.touch();
        }
    }

    pub fn close_buffer(&mut self, buffer_id: BufferId) -> Option<BufferId> {
        let network_id = self.buffer(buffer_id)?.network;
        self.buffers.remove(&buffer_id);
        if let Some(network) = self.network_mut(network_id) {
            network.buffers.retain(|id| *id != buffer_id);
        }
        if self.active == Some(buffer_id) {
            self.active = self
                .network(network_id)
                .and_then(|network| {
                    network
                        .buffers
                        .iter()
                        .copied()
                        .find(|id| self.buffers.contains_key(id))
                })
                .or_else(|| self.buffers.keys().next().copied());
        }
        self.touch();
        self.active
    }

    pub fn set_topic(
        &mut self,
        buffer_id: BufferId,
        topic: impl Into<String>,
        by: Option<String>,
        at: Option<DateTime<Local>>,
    ) {
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.topic = topic.into();
            buffer.topic_by = by;
            buffer.topic_at = at;
            self.touch();
        }
    }

    pub fn clear_users(&mut self, buffer_id: BufferId) {
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.users.clear();
            buffer.typing.clear();
            self.touch();
        }
    }

    pub fn mark_all_parted(&mut self, network_id: NetworkId) {
        let ids: Vec<BufferId> = self
            .network(network_id)
            .map(|network| network.buffers.clone())
            .unwrap_or_default();
        for id in ids {
            if let Some(buffer) = self.buffer_mut(id) {
                if buffer.kind == BufferKind::Channel {
                    buffer.joined = false;
                    buffer.users.clear();
                    buffer.typing.clear();
                }
            }
        }
        self.touch();
    }

    pub fn rename_server_buffer(&mut self, network_id: NetworkId, name: impl Into<String>) {
        let name = name.into();
        if let Some(server) = self.server_buffer(network_id) {
            if let Some(buffer) = self.buffer_mut(server) {
                buffer.name = name;
                self.touch();
            }
        }
    }

    pub fn set_modes(&mut self, buffer_id: BufferId, modes: impl Into<String>) {
        if let Some(buffer) = self.buffer_mut(buffer_id) {
            buffer.modes = modes.into();
            self.touch();
        }
    }

    pub fn set_network_status(
        &mut self,
        network_id: NetworkId,
        status: ConnectionStatus,
        text: impl Into<String>,
    ) {
        if let Some(network) = self.network_mut(network_id) {
            network.status = status;
            network.status_text = text.into();
            if status == ConnectionStatus::Connected {
                network.connected_at = Some(Local::now());
                network.last_error = None;
            }
            if status == ConnectionStatus::Disconnected && !network.status_text.is_empty() {
                network.last_error = Some(network.status_text.clone());
            }
            self.touch();
        }
    }

    pub fn set_network_nick(&mut self, network_id: NetworkId, nick: impl Into<String>) {
        if let Some(network) = self.network_mut(network_id) {
            network.nick = nick.into();
            self.touch();
        }
    }

    pub fn set_network_name(&mut self, network_id: NetworkId, name: impl Into<String>) {
        if let Some(network) = self.network_mut(network_id) {
            let name = name.into();
            if !name.is_empty() {
                network.name = name;
            }
            self.touch();
        }
    }

    pub fn set_caps(&mut self, network_id: NetworkId, caps: Vec<String>) {
        if let Some(network) = self.network_mut(network_id) {
            network.caps = caps;
            self.touch();
        }
    }

    pub fn set_lag(&mut self, network_id: NetworkId, lag_ms: Option<u64>) {
        if let Some(network) = self.network_mut(network_id) {
            network.lag_ms = lag_ms;
            self.touch();
        }
    }

    pub fn set_away(&mut self, network_id: NetworkId, away: bool) {
        if let Some(network) = self.network_mut(network_id) {
            network.away = away;
            self.touch();
        }
    }

    /// Met à jour un utilisateur dans tous les tampons du réseau.
    pub fn for_user_buffers<F>(&mut self, network_id: NetworkId, nick: &str, mut f: F)
    where
        F: FnMut(&mut User),
    {
        let ids: Vec<BufferId> = self
            .network(network_id)
            .map(|network| network.buffers.clone())
            .unwrap_or_default();
        for id in ids {
            if let Some(user) = self.buffer_mut(id).and_then(|buffer| buffer.user_mut(nick)) {
                f(user);
            }
        }
    }

    pub fn rename_user_everywhere(&mut self, network_id: NetworkId, old: &str, new: &str) {
        let ids: Vec<BufferId> = self
            .network(network_id)
            .map(|network| network.buffers.clone())
            .unwrap_or_default();
        for id in ids {
            if let Some(buffer) = self.buffer_mut(id) {
                if buffer.kind == BufferKind::Query && irc_fold(&buffer.name) == irc_fold(old) {
                    buffer.name = new.to_string();
                }
                buffer.rename_user(old, new);
            }
        }
        if let Some(network) = self.network_mut(network_id) {
            if irc_fold(&network.nick) == irc_fold(old) {
                network.nick = new.to_string();
            }
        }
        self.touch();
    }

    pub fn remove_user_everywhere(&mut self, network_id: NetworkId, nick: &str) -> Vec<BufferId> {
        let ids: Vec<BufferId> = self
            .network(network_id)
            .map(|network| network.buffers.clone())
            .unwrap_or_default();
        let mut touched = Vec::new();
        for id in ids {
            if let Some(buffer) = self.buffer_mut(id) {
                if buffer.remove_user(nick).is_some() {
                    touched.push(id);
                }
            }
        }
        self.touch();
        touched
    }

    pub fn user_buffers(&self, network_id: NetworkId, nick: &str) -> Vec<BufferId> {
        let folded = irc_fold(nick);
        self.network(network_id)
            .map(|network| {
                network
                    .buffers
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.buffers.get(id).is_some_and(|buffer| {
                            buffer
                                .users
                                .iter()
                                .any(|user| irc_fold(&user.nick) == folded)
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn query_buffers(&self, network_id: NetworkId) -> Vec<BufferId> {
        self.network(network_id)
            .map(|network| {
                network
                    .buffers
                    .iter()
                    .copied()
                    .filter(|id| {
                        self.buffers
                            .get(id)
                            .is_some_and(|buffer| buffer.kind == BufferKind::Query)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn total_unread(&self) -> u32 {
        self.buffers.values().map(|buffer| buffer.unread).sum()
    }

    pub fn highlighted(&self) -> bool {
        self.buffers.values().any(|buffer| buffer.highlight)
    }

    pub fn start_channel_list(&mut self, network_id: NetworkId) {
        self.channel_lists.insert(
            network_id,
            ChannelListState {
                loading: true,
                complete: false,
                ..Default::default()
            },
        );
        self.touch();
    }

    pub fn add_channel_entry(
        &mut self,
        network_id: NetworkId,
        channel: impl Into<String>,
        users: usize,
        topic: impl Into<String>,
    ) {
        let channel = channel.into();
        let entry = ChannelListEntry {
            channel: channel.clone(),
            users,
            topic: topic.into(),
        };
        let state = self.channel_lists.entry(network_id).or_default();
        if let Some(existing) = state
            .entries
            .iter_mut()
            .find(|entry| irc_fold(&entry.channel) == irc_fold(&channel))
        {
            *existing = entry;
        } else {
            state.entries.push(entry);
        }
        self.touch();
    }

    pub fn finish_channel_list(&mut self, network_id: NetworkId) {
        if let Some(state) = self.channel_lists.get_mut(&network_id) {
            state.loading = false;
            state.complete = true;
            state.updated_at = Some(Local::now());
            state.entries.sort_by(|a, b| {
                b.users
                    .cmp(&a.users)
                    .then_with(|| irc_fold(&a.channel).cmp(&irc_fold(&b.channel)))
            });
        }
        self.touch();
    }

    pub fn channel_list(&self, network_id: NetworkId) -> Option<&ChannelListState> {
        self.channel_lists.get(&network_id)
    }

    pub fn add_dcc(&mut self, mut transfer: DccTransfer) -> u64 {
        let id = self.next_dcc_id;
        self.next_dcc_id = self.next_dcc_id.wrapping_add(1).max(1);
        transfer.id = id;
        self.dcc.push(transfer);
        if self.dcc.len() > 50 {
            self.dcc.retain(|transfer| transfer.is_active());
        }
        self.touch();
        id
    }

    pub fn dcc_transfer(&self, id: u64) -> Option<&DccTransfer> {
        self.dcc.iter().find(|transfer| transfer.id == id)
    }

    pub fn dcc_transfer_mut(&mut self, id: u64) -> Option<&mut DccTransfer> {
        self.dcc.iter_mut().find(|transfer| transfer.id == id)
    }

    pub fn cancel_dcc(&mut self, id: Option<u64>) {
        for transfer in &mut self.dcc {
            if transfer.is_active() && id.is_none_or(|wanted| transfer.id == wanted) {
                transfer.request_cancel();
            }
        }
        self.touch();
    }

    pub fn clear_finished_dcc(&mut self) {
        self.dcc.retain(|transfer| transfer.is_active());
        self.touch();
    }

    pub fn dcc_for_network(&self, network: NetworkId) -> Vec<DccTransfer> {
        self.dcc
            .iter()
            .filter(|transfer| transfer.network == network)
            .cloned()
            .collect()
    }

    pub fn save_config(&self) {
        if let Err(error) = self.config.save() {
            log::warn!("sauvegarde de la configuration impossible : {error}");
        }
    }

    pub fn window_title(&self) -> String {
        let mut title = String::from(crate::APP_NAME);
        if let Some(buffer) = self.active_buffer() {
            title.push_str(" — ");
            title.push_str(&buffer.name);
            if let Some(network) = self.network(buffer.network) {
                if network.status != ConnectionStatus::Connected {
                    title.push_str(" [hors ligne]");
                }
            }
        }
        let unread = self.total_unread();
        if unread > 0 {
            title.push_str(&format!(" ({unread})"));
        }
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ConnectionConfig {
        ConnectionConfig {
            host: "irc.test".into(),
            port: 6697,
            tls: true,
            verify_tls: true,
            nick: "moi".into(),
            user: "moi".into(),
            realname: "Moi".into(),
            sasl_account: None,
            sasl_password: None,
            channels: vec!["#test".into()],
            auto_reconnect: true,
        }
    }

    #[test]
    fn create_network_and_buffers() {
        let mut model = AppModel::new(Config::default());
        let id = model.add_network(config());
        let server = model.server_buffer(id).unwrap();
        assert_eq!(model.active, Some(server));
        let channel = model.ensure_buffer(id, BufferKind::Channel, "#test");
        assert_eq!(
            model.find_buffer(id, BufferKind::Channel, "#TEST"),
            Some(channel)
        );
        assert_eq!(model.network(id).unwrap().buffers.len(), 2);
    }

    #[test]
    fn unread_and_active() {
        let mut model = AppModel::new(Config::default());
        let id = model.add_network(config());
        let channel = model.ensure_buffer(id, BufferKind::Channel, "#test");
        let mut line = ChatLine::new(LineKind::Message, Some("alice".into()), "salut moi");
        line.mention = true;
        model.append_line(channel, line);
        assert_eq!(model.buffer(channel).unwrap().unread, 1);
        assert!(model.buffer(channel).unwrap().highlight);
        model.set_active(channel);
        assert_eq!(model.buffer(channel).unwrap().unread, 0);
        assert!(!model.buffer(channel).unwrap().highlight);
    }

    #[test]
    fn rename_user_updates_query() {
        let mut model = AppModel::new(Config::default());
        let id = model.add_network(config());
        let query = model.ensure_buffer(id, BufferKind::Query, "alice");
        model
            .buffer_mut(query)
            .unwrap()
            .users
            .push(User::new("alice"));
        model.rename_user_everywhere(id, "alice", "robert");
        assert_eq!(model.buffer(query).unwrap().name, "robert");
        assert!(model.buffer(query).unwrap().user("robert").is_some());
    }

    #[test]
    fn sorting_users() {
        let mut buffer = Buffer::new(1, 1, BufferKind::Channel, "#x");
        let mut op = User::new("zeta");
        op.add_prefix('@');
        buffer.add_or_update_user(op);
        buffer.add_or_update_user(User::new("alpha"));
        let mut voice = User::new("beta");
        voice.add_prefix('+');
        buffer.add_or_update_user(voice);
        let names: Vec<_> = buffer.users_sorted().into_iter().map(|u| u.nick).collect();
        assert_eq!(names, vec!["zeta", "beta", "alpha"]);
    }

    #[test]
    fn cap_messages() {
        let mut buffer = Buffer::new(1, 1, BufferKind::Channel, "#x");
        for index in 0..(MAX_MESSAGES + 10) {
            buffer.add_message(ChatLine::new(
                LineKind::Message,
                None,
                format!("message {index}"),
            ));
        }
        assert_eq!(buffer.messages.len(), MAX_MESSAGES);
        assert!(buffer.messages.front().unwrap().text.contains("10"));
    }
}
