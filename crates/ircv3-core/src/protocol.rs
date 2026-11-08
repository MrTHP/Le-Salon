//! Logique de session IRCv3 : négociation CAP/SASL, numerics, messages,
//! modes, listes de pseudos, commandes utilisateur.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::Engine as _;
use chrono::Local;

use crate::i18n::t;

macro_rules! lmsg {
    ($fr:expr, $en:expr) => {
        if crate::i18n::lang() == crate::i18n::Lang::Fr {
            $fr
        } else {
            $en
        }
    };
}

use crate::{
    APP_NAME, VERSION, Wake,
    commands::{self, ActiveBuffer, LocalCommand, ParsedCommand},
    config::ConnectionConfig,
    irc_fold, mentions,
    message::{Message, is_channel_name},
    model::{
        AppModel, BufferId, BufferKind, ChatLine, ConnectionStatus, LineKind, NetworkId, User,
    },
};

const KEEPALIVE: Duration = Duration::from_secs(90);
const TIMEOUT: Duration = Duration::from_secs(300);
const CAP_TIMEOUT: Duration = Duration::from_secs(10);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(90);

const REQUESTED_CAPS: &[&str] = &[
    "message-tags",
    "server-time",
    "echo-message",
    "multi-prefix",
    "cap-notify",
    "account-notify",
    "account-tag",
    "away-notify",
    "batch",
    "chathistory",
    "extended-join",
    "invite-notify",
    "message-ids",
    "monitor",
    "setname",
    "userhost-in-names",
    "typing",
    "chghost",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionAction {
    #[default]
    None,
    Reconnect,
    Disconnect,
}

#[derive(Debug, Default)]
pub struct UserOutcome {
    pub lines: Vec<String>,
    pub action: ConnectionAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaslState {
    None,
    WaitChallenge,
    WaitResult,
}

#[derive(Clone, Debug)]
struct Batch {
    id: String,
    kind: String,
}

pub struct Session {
    pub network_id: NetworkId,
    pub config: ConnectionConfig,
    model: Arc<Mutex<AppModel>>,
    wake: Wake,
    pub nick: String,
    caps_available: HashSet<String>,
    caps_acked: HashSet<String>,
    cap_ls_received: bool,
    cap_req_sent: bool,
    cap_end_sent: bool,
    cap_sent_at: Instant,
    sasl: SaslState,
    pub registered: bool,
    pub ever_registered: bool,
    registered_at: Option<Instant>,
    ping_token: Option<String>,
    ping_sent_at: Option<Instant>,
    last_ping: Instant,
    last_received: Instant,
    next_ping: u64,
    batches: Vec<Batch>,
    join_sent: bool,
    nick_retries: u8,
}

impl Session {
    pub fn new(
        network_id: NetworkId,
        config: ConnectionConfig,
        model: Arc<Mutex<AppModel>>,
        wake: Wake,
    ) -> Self {
        let now = Instant::now();
        Self {
            network_id,
            nick: config.nick.clone(),
            config,
            model,
            wake,
            caps_available: HashSet::new(),
            caps_acked: HashSet::new(),
            cap_ls_received: false,
            cap_req_sent: false,
            cap_end_sent: false,
            cap_sent_at: now,
            sasl: SaslState::None,
            registered: false,
            ever_registered: false,
            registered_at: None,
            ping_token: None,
            ping_sent_at: None,
            last_ping: now,
            last_received: now,
            next_ping: 0,
            batches: Vec::new(),
            join_sent: false,
            nick_retries: 0,
        }
    }

    fn with_model<R>(&self, f: impl FnOnce(&mut AppModel) -> R) -> R {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut model)
    }

    fn wake(&self) {
        let _ = self.wake.try_send(());
    }

    pub fn set_status(&self, status: ConnectionStatus, text: impl Into<String>) {
        self.with_model(|model| model.set_network_status(self.network_id, status, text));
        self.wake();
    }

    pub fn note_connecting(&self) {
        self.set_status(
            ConnectionStatus::Connecting,
            lmsg!(
                format!("connexion à {}…", self.config.endpoint()),
                format!("connecting to {}…", self.config.endpoint())
            ),
        );
        self.server_line(
            LineKind::Info,
            lmsg!(
                format!(
                    "Connexion à {}{}…",
                    self.config.endpoint(),
                    if self.config.tls { " (TLS)" } else { "" }
                ),
                format!(
                    "Connecting to {}{}…",
                    self.config.endpoint(),
                    if self.config.tls { " (TLS)" } else { "" }
                )
            ),
        );
    }

    pub fn on_connected(&mut self) {
        self.last_received = Instant::now();
        self.set_status(
            ConnectionStatus::Registering,
            t("négociation des capacités…", "negotiating capabilities…"),
        );
    }

    pub fn initial_lines(&mut self) -> Vec<String> {
        self.cap_sent_at = Instant::now();
        self.last_ping = Instant::now();
        let mut out = Vec::new();
        out.push("CAP LS 302".to_string());
        out.push(format!("NICK {}", self.nick));
        out.push(format!(
            "USER {} 0 * :{}",
            self.config.user, self.config.realname
        ));
        out
    }

    pub fn reset_for_reconnect(&mut self) {
        self.nick = self.config.nick.clone();
        self.caps_available.clear();
        self.caps_acked.clear();
        self.cap_ls_received = false;
        self.cap_req_sent = false;
        self.cap_end_sent = false;
        self.cap_sent_at = Instant::now();
        self.sasl = SaslState::None;
        self.registered = false;
        self.registered_at = None;
        self.ping_token = None;
        self.ping_sent_at = None;
        self.last_ping = Instant::now();
        self.last_received = Instant::now();
        self.batches.clear();
        self.join_sent = false;
        self.nick_retries = 0;
        self.with_model(|model| model.mark_all_parted(self.network_id));
        self.set_status(
            ConnectionStatus::Connecting,
            t("reconnexion…", "reconnecting…"),
        );
    }

    pub fn on_disconnected(&mut self, reason: Option<String>) {
        let reason =
            reason.unwrap_or_else(|| t("connexion fermée", "connection closed").to_string());
        self.registered = false;
        self.join_sent = false;
        self.with_model(|model| {
            model.mark_all_parted(self.network_id);
            model.set_caps(self.network_id, Vec::new());
            model.set_lag(self.network_id, None);
        });
        self.set_status(ConnectionStatus::Disconnected, reason.clone());
        self.server_line(
            LineKind::Error,
            lmsg!(
                format!("Déconnecté : {reason}"),
                format!("Disconnected: {reason}")
            ),
        );
    }

    pub fn timed_out(&self) -> bool {
        if self.registered {
            self.last_received.elapsed() > TIMEOUT
        } else {
            self.registered_at
                .map(|at| at.elapsed() > REGISTRATION_TIMEOUT)
                .unwrap_or_else(|| self.last_received.elapsed() > REGISTRATION_TIMEOUT)
        }
    }

    pub fn tick(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.cap_end_sent && self.cap_sent_at.elapsed() > CAP_TIMEOUT {
            self.cap_end(&mut out);
        }
        if self.registered && self.last_ping.elapsed() > KEEPALIVE {
            self.next_ping += 1;
            let token = format!("{}-{}", self.config.nick, self.next_ping);
            self.ping_token = Some(token.clone());
            self.ping_sent_at = Some(Instant::now());
            self.last_ping = Instant::now();
            out.push(format!("PING :{token}"));
        }
        out
    }

    fn server_buffer(&self) -> BufferId {
        self.with_model(|model| model.server_buffer(self.network_id).unwrap_or(0))
    }

    fn server_line(&self, kind: LineKind, text: impl Into<String>) {
        let buffer = self.server_buffer();
        self.append(buffer, kind, None, text, false, false, None);
    }

    fn active_buffer_id(&self) -> Option<BufferId> {
        self.with_model(|model| {
            model
                .active_buffer()
                .filter(|buffer| buffer.network == self.network_id)
                .map(|buffer| buffer.id)
        })
    }

    fn active_or_server(&self) -> BufferId {
        self.active_buffer_id()
            .unwrap_or_else(|| self.server_buffer())
    }

    fn existing_buffer(&self, kind: BufferKind, name: &str) -> Option<BufferId> {
        self.with_model(|model| model.find_buffer(self.network_id, kind, name))
    }

    fn channel_buffer(&self, channel: &str) -> BufferId {
        let channel = strip_statusmsg(channel);
        self.with_model(|model| model.ensure_buffer(self.network_id, BufferKind::Channel, channel))
    }

    fn query_buffer(&self, nick: &str) -> BufferId {
        self.with_model(|model| model.ensure_buffer(self.network_id, BufferKind::Query, nick))
    }

    fn append(
        &self,
        buffer_id: BufferId,
        kind: LineKind,
        nick: Option<String>,
        text: impl Into<String>,
        mention: bool,
        self_nick: bool,
        at: Option<chrono::DateTime<Local>>,
    ) {
        if buffer_id == 0 {
            return;
        }
        let mut line = ChatLine::new(kind, nick, text);
        if let Some(at) = at {
            line = line.with_time(at);
        }
        line.mention = mention;
        line.self_nick = self_nick;
        self.with_model(|model| model.append_line(buffer_id, line));
        self.wake();
    }

    fn echo_message(&self) -> bool {
        self.caps_acked.contains("echo-message")
    }

    fn line_kind(&self, msg: &Message, default: LineKind) -> LineKind {
        let Some(batch_id) = msg.tag("batch") else {
            return default;
        };
        if self.batches.iter().any(|batch| {
            batch.id == batch_id && matches!(batch.kind.as_str(), "chathistory" | "history")
        }) {
            if default == LineKind::Message {
                LineKind::History
            } else {
                default
            }
        } else {
            default
        }
    }

    pub fn on_line(&mut self, line: &str) -> Vec<String> {
        let message = match Message::parse(line) {
            Ok(message) => message,
            Err(error) => {
                self.server_line(
                    LineKind::Error,
                    lmsg!(
                        format!("Ligne IRC illisible : {error}"),
                        format!("Unreadable IRC line: {error}")
                    ),
                );
                return Vec::new();
            }
        };
        self.last_received = Instant::now();
        if let Ok(numeric) = message.command.parse::<u16>() {
            return self.on_numeric(numeric, &message);
        }
        match message.command.as_str() {
            "PING" => {
                let token = message.params.last().cloned().unwrap_or_default();
                vec![format!("PONG :{token}")]
            }
            "PONG" => {
                if self
                    .ping_token
                    .as_deref()
                    .is_some_and(|token| message.params.iter().any(|param| param == token))
                {
                    if let Some(sent) = self.ping_sent_at.take() {
                        let lag = sent.elapsed().as_millis() as u64;
                        self.with_model(|model| model.set_lag(self.network_id, Some(lag)));
                    }
                    self.ping_token = None;
                }
                Vec::new()
            }
            "CAP" => self.on_cap(&message),
            "AUTHENTICATE" => self.on_authenticate(&message),
            "JOIN" => self.on_join(&message),
            "PART" => self.on_part(&message),
            "QUIT" => self.on_quit(&message),
            "NICK" => self.on_nick(&message),
            "PRIVMSG" => self.on_privmsg(&message),
            "NOTICE" => self.on_notice(&message),
            "TAGMSG" => self.on_tagmsg(&message),
            "TOPIC" => self.on_topic(&message),
            "MODE" => self.on_mode(&message),
            "KICK" => self.on_kick(&message),
            "INVITE" => self.on_invite(&message),
            "AWAY" => self.on_away(&message),
            "ACCOUNT" => self.on_account(&message),
            "CHGHOST" => self.on_chghost(&message),
            "SETNAME" => self.on_setname(&message),
            "BATCH" => self.on_batch(&message),
            "FAIL" => self.on_fail(&message, LineKind::Error, "FAIL"),
            "WARN" => self.on_fail(&message, LineKind::Error, "WARN"),
            "NOTE" => self.on_fail(&message, LineKind::Info, "NOTE"),
            "ERROR" => {
                let text = message.params.join(" ");
                self.server_line(
                    LineKind::Error,
                    lmsg!(
                        format!("Erreur serveur : {text}"),
                        format!("Server error: {text}")
                    ),
                );
                self.set_status(ConnectionStatus::Disconnected, text);
                Vec::new()
            }
            "WALLOPS" => {
                let text = message.params.join(" ");
                self.server_line(LineKind::Info, format!("WALLOPS : {text}"));
                Vec::new()
            }
            "KILL" => {
                let text = message.params.join(" ");
                self.server_line(LineKind::Error, format!("KILL : {text}"));
                Vec::new()
            }
            _ => {
                log::debug!("commande IRC non gérée : {}", message.command);
                Vec::new()
            }
        }
    }

    fn on_cap(&mut self, msg: &Message) -> Vec<String> {
        let sub = msg.param(1).unwrap_or("").to_ascii_uppercase();
        let caps_text = msg.params.last().map(String::as_str).unwrap_or("");
        let multiline = msg.param(2).is_some_and(|value| value == "*");
        let mut out = Vec::new();
        match sub.as_str() {
            "LS" => {
                for cap in caps_text.split_whitespace() {
                    let name = cap.split('=').next().unwrap_or(cap);
                    if !name.is_empty() {
                        self.caps_available.insert(name.to_string());
                    }
                }
                if multiline {
                    return out;
                }
                self.cap_ls_received = true;
                out.extend(self.request_capabilities());
            }
            "ACK" => {
                for cap in caps_text.split_whitespace() {
                    let name = cap.split('=').next().unwrap_or(cap);
                    if !name.is_empty() {
                        self.caps_acked.insert(name.to_string());
                    }
                }
                let mut caps: Vec<_> = self.caps_acked.iter().cloned().collect();
                caps.sort();
                self.with_model(|model| model.set_caps(self.network_id, caps.clone()));
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("Capacités activées : {}", caps.join(" ")),
                        format!("Enabled capabilities: {}", caps.join(" "))
                    ),
                );
                if self.caps_acked.contains("sasl") && self.config.has_sasl() {
                    self.start_sasl(&mut out);
                } else {
                    self.cap_end(&mut out);
                }
            }
            "NAK" => {
                self.server_line(
                    LineKind::Error,
                    lmsg!(
                        format!("Capacités refusées par le serveur : {caps_text}"),
                        format!("Capabilities rejected by server: {caps_text}")
                    ),
                );
                self.cap_end(&mut out);
            }
            "NEW" => {
                let mut added = Vec::new();
                for cap in caps_text.split_whitespace() {
                    let name = cap.split('=').next().unwrap_or(cap);
                    if self.caps_available.insert(name.to_string()) {
                        added.push(name.to_string());
                    }
                }
                if !added.is_empty() {
                    self.server_line(
                        LineKind::Info,
                        lmsg!(
                            format!("Nouvelles capacités : {}", added.join(" ")),
                            format!("New capabilities: {}", added.join(" "))
                        ),
                    );
                }
            }
            "DEL" => {
                for cap in caps_text.split_whitespace() {
                    self.caps_available.remove(cap);
                    self.caps_acked.remove(cap);
                }
            }
            _ => {}
        }
        out
    }

    fn request_capabilities(&mut self) -> Vec<String> {
        if self.cap_req_sent {
            return Vec::new();
        }
        self.cap_req_sent = true;
        let available_empty = self.caps_available.is_empty();
        let mut requested: Vec<String> = REQUESTED_CAPS
            .iter()
            .filter(|cap| available_empty || self.caps_available.contains(**cap))
            .map(|cap| (*cap).to_string())
            .collect();
        if self.config.has_sasl() {
            if available_empty || self.caps_available.contains("sasl") {
                requested.push("sasl".into());
            } else {
                self.server_line(
                    LineKind::Error,
                    t(
                        "SASL demandé mais non proposé par le serveur ; connexion sans SASL.",
                        "SASL requested but not offered by server; connecting without SASL.",
                    ),
                );
            }
        }
        if requested.is_empty() {
            let mut out = Vec::new();
            self.cap_end(&mut out);
            return out;
        }
        vec![format!("CAP REQ :{}", requested.join(" "))]
    }

    fn start_sasl(&mut self, out: &mut Vec<String>) {
        if self.sasl == SaslState::None && self.config.has_sasl() {
            self.sasl = SaslState::WaitChallenge;
            out.push("AUTHENTICATE PLAIN".to_string());
        }
    }

    fn on_authenticate(&mut self, msg: &Message) -> Vec<String> {
        let payload = msg.param(0).unwrap_or("");
        let mut out = Vec::new();
        if payload == "+" && self.sasl == SaslState::WaitChallenge {
            let account = self.config.sasl_account.clone().unwrap_or_default();
            let password = self.config.sasl_password.clone().unwrap_or_default();
            let raw = format!("{account}\0{account}\0{password}");
            let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
            for chunk in encoded.as_bytes().chunks(400) {
                out.push(format!(
                    "AUTHENTICATE {}",
                    std::str::from_utf8(chunk).unwrap_or_default()
                ));
            }
            if encoded.len() % 400 == 0 {
                out.push("AUTHENTICATE +".to_string());
            }
            self.sasl = SaslState::WaitResult;
        } else if payload != "+" {
            self.server_line(
                LineKind::Error,
                lmsg!(
                    format!("SASL : défi inattendu ({payload})."),
                    format!("SASL: unexpected challenge ({payload}).")
                ),
            );
            self.sasl = SaslState::None;
            self.cap_end(&mut out);
        }
        out
    }

    fn cap_end(&mut self, out: &mut Vec<String>) {
        if !self.cap_end_sent {
            self.cap_end_sent = true;
            out.push("CAP END".to_string());
        }
    }

    fn on_numeric(&mut self, numeric: u16, msg: &Message) -> Vec<String> {
        let mut out = Vec::new();
        match numeric {
            1 => {
                if let Some(nick) = msg.param(0) {
                    self.nick = nick.to_string();
                    self.with_model(|model| {
                        model.set_network_nick(self.network_id, nick);
                    });
                }
                self.registered = true;
                self.ever_registered = true;
                self.registered_at = Some(Instant::now());
                self.with_model(|model| {
                    let host = model
                        .network(self.network_id)
                        .map(|network| network.host.clone())
                        .unwrap_or_default();
                    model.set_network_name(self.network_id, host);
                });
                self.set_status(ConnectionStatus::Connected, t("connecté", "connected"));
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("Connecté en tant que {}.", self.nick),
                        format!("Connected as {}.", self.nick)
                    ),
                );
                self.cap_end(&mut out);
                if !self.join_sent && !self.config.channels.is_empty() {
                    self.join_sent = true;
                    out.push(format!("JOIN {}", self.config.channels.join(",")));
                }
            }
            2 | 3 => {
                let text = trailing_after(msg, 1);
                if !text.is_empty() {
                    self.server_line(LineKind::Server, text);
                }
            }
            4 => {
                let text = trailing_after(msg, 1);
                if !text.is_empty() {
                    self.server_line(LineKind::Server, format!("Serveur : {text}"));
                }
            }
            5 => {
                let mut network_name = None;
                let mut prefix_modes = None;
                let mut chantypes = None;
                let mut casemapping = None;
                for token in msg.params.iter().skip(1) {
                    if let Some(value) = token.strip_prefix("NETWORK=") {
                        network_name = Some(value.to_string());
                    } else if let Some(value) = token.strip_prefix("PREFIX=") {
                        prefix_modes = Some(parse_prefix(value));
                    } else if let Some(value) = token.strip_prefix("CHANTYPES=") {
                        chantypes = Some(value.to_string());
                    } else if let Some(value) = token.strip_prefix("CASEMAPPING=") {
                        casemapping = Some(value.to_string());
                    }
                }
                self.with_model(|model| {
                    if let Some(network) = model.network_mut(self.network_id) {
                        if let Some(name) = network_name {
                            network.name = name;
                        }
                        if let Some(modes) = prefix_modes {
                            network.prefix_modes = modes;
                        }
                        if let Some(types) = chantypes {
                            network.chantypes = types;
                        }
                        if let Some(mapping) = casemapping {
                            network.casemapping = mapping;
                        }
                    }
                    let details = model.network(self.network_id).map(|network| {
                        let prefixes: String = network
                            .prefix_modes
                            .iter()
                            .map(|(_, prefix)| *prefix)
                            .collect();
                        (
                            network.display_name().to_string(),
                            network.host.clone(),
                            prefixes,
                        )
                    });
                    if let Some((name, host, prefixes)) = details {
                        model.set_network_name(self.network_id, name.clone());
                        model.rename_server_buffer(self.network_id, name.clone());
                        let server = model.server_buffer(self.network_id).unwrap_or(0);
                        model.append_line(
                            server,
                            ChatLine::new(
                                LineKind::Info,
                                None,
                                lmsg!(
                                    format!("Réseau {name} ({host}) — préfixes {prefixes}"),
                                    format!("Network {name} ({host}) — prefixes {prefixes}")
                                ),
                            ),
                        );
                    }
                    model.touch();
                });
                self.wake();
            }
            250 | 251 | 252 | 253 | 254 | 255 | 265 | 266 => {
                let text = trailing_after(msg, 1);
                if !text.is_empty() {
                    self.server_line(LineKind::Server, text);
                }
            }
            301 => {
                let nick = msg.param(1).unwrap_or("?");
                let text = msg.param(2).unwrap_or("");
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("{nick} est absent : {text}"),
                        format!("{nick} is away: {text}")
                    ),
                );
            }
            305 => {
                self.with_model(|model| model.set_away(self.network_id, false));
                self.server_line(
                    LineKind::Info,
                    t("Vous n'êtes plus absent.", "You are no longer away."),
                );
            }
            306 => {
                self.with_model(|model| model.set_away(self.network_id, true));
                self.server_line(
                    LineKind::Info,
                    t("Vous êtes maintenant absent.", "You are now away."),
                );
            }
            307 => {
                let nick = msg.param(1).unwrap_or(&self.nick);
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("{nick} est enregistré."),
                        format!("{nick} is registered.")
                    ),
                );
            }
            311 => {
                let nick = msg.param(1).unwrap_or("?");
                let user = msg.param(2).unwrap_or("");
                let host = msg.param(3).unwrap_or("");
                let realname = msg.param(5).unwrap_or("");
                self.whois_line(lmsg!(
                    format!("{nick} ({user}@{host}) : {realname}"),
                    format!("{nick} ({user}@{host}): {realname}")
                ));
            }
            312 => {
                let nick = msg.param(1).unwrap_or("?");
                let server = msg.param(2).unwrap_or("");
                let info = msg.param(3).unwrap_or("");
                self.whois_line(lmsg!(
                    format!("{nick} sur {server} : {info}"),
                    format!("{nick} on {server}: {info}")
                ));
            }
            313 => {
                let nick = msg.param(1).unwrap_or("?");
                let text = msg.param(2).unwrap_or("");
                self.whois_line(format!("{nick} : {text}"));
            }
            315 => {
                let mask = msg.param(1).unwrap_or("");
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("Fin de /WHO {mask}."),
                        format!("End of /WHO {mask}.")
                    ),
                );
            }
            317 => {
                let nick = msg.param(1).unwrap_or("?");
                let idle = msg.param(2).unwrap_or("0");
                self.whois_line(lmsg!(
                    format!("{nick} : inactif depuis {idle} s."),
                    format!("{nick}: idle for {idle}s.")
                ));
            }
            318 => {
                let nick = msg.param(1).unwrap_or("");
                self.whois_line(lmsg!(
                    format!("Fin de /WHOIS {nick}."),
                    format!("End of /WHOIS {nick}.")
                ));
            }
            319 => {
                let nick = msg.param(1).unwrap_or("?");
                let channels = msg.param(2).unwrap_or("");
                self.whois_line(lmsg!(
                    format!("{nick} est sur : {channels}"),
                    format!("{nick} is on: {channels}")
                ));
            }
            321 => {
                self.with_model(|model| model.start_channel_list(self.network_id));
                self.wake();
            }
            322 => {
                let channel = msg.param(1).unwrap_or("").to_string();
                let users = msg
                    .param(2)
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                let topic = msg.param(3).unwrap_or("").to_string();
                if !channel.is_empty() {
                    self.with_model(|model| {
                        model.add_channel_entry(self.network_id, channel, users, topic);
                    });
                    self.wake();
                }
            }
            323 => {
                let count = self.with_model(|model| {
                    model
                        .channel_list(self.network_id)
                        .map(|state| state.entries.len())
                        .unwrap_or(0)
                });
                self.with_model(|model| model.finish_channel_list(self.network_id));
                self.wake();
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("Fin de /LIST : {count} salons."),
                        format!("End of /LIST: {count} channels.")
                    ),
                );
            }
            324 => {
                let channel = msg.param(1).unwrap_or("");
                let modes = msg.param(2).unwrap_or("");
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    self.with_model(|model| model.set_modes(buffer, modes));
                }
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("Modes de {channel} : {modes}"),
                        format!("Modes for {channel}: {modes}")
                    ),
                );
            }
            329 => {
                let channel = msg.param(1).unwrap_or("");
                let stamp = msg.param(2).unwrap_or("");
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    if let Ok(seconds) = stamp.parse::<i64>() {
                        if let Some(at) = chrono::DateTime::from_timestamp(seconds, 0) {
                            self.with_model(|model| {
                                if let Some(buffer) = model.buffer_mut(buffer) {
                                    buffer.created_at = Some(at.with_timezone(&Local));
                                }
                            });
                        }
                    }
                }
            }
            331 => {
                let channel = msg.param(1).unwrap_or("");
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    self.with_model(|model| model.set_topic(buffer, "", None, None));
                }
            }
            332 => {
                let channel = msg.param(1).unwrap_or("");
                let topic = msg.param(2).unwrap_or("");
                let buffer = self.channel_buffer(channel);
                self.with_model(|model| model.set_topic(buffer, topic, None, None));
                self.append(
                    buffer,
                    LineKind::Topic,
                    None,
                    lmsg!(format!("Sujet : {topic}"), format!("Topic: {topic}")),
                    false,
                    false,
                    msg.server_time(),
                );
            }
            333 => {
                let channel = msg.param(1).unwrap_or("");
                let by = msg.param(2).unwrap_or("");
                let stamp = msg.param(3).unwrap_or("");
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    let at = stamp
                        .parse::<i64>()
                        .ok()
                        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
                        .map(|at| at.with_timezone(&Local));
                    self.with_model(|model| {
                        if let Some(buffer) = model.buffer_mut(buffer) {
                            buffer.topic_by = Some(by.to_string());
                            buffer.topic_at = at;
                        }
                    });
                }
            }
            352 => {
                let channel = strip_statusmsg(msg.param(1).unwrap_or(""));
                let user_name = msg.param(2).unwrap_or("");
                let host = msg.param(3).unwrap_or("");
                let nick = msg.param(5).unwrap_or("");
                let flags = msg.param(6).unwrap_or("");
                let realname = msg.param(7).unwrap_or("");
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    self.with_model(|model| {
                        if let Some(buffer) = model.buffer_mut(buffer) {
                            if let Some(user) = buffer.user_mut(nick) {
                                user.user = Some(user_name.to_string());
                                user.host = Some(host.to_string());
                                user.realname = Some(realname.to_string());
                                user.away = flags.contains('G');
                            }
                        }
                    });
                }
            }
            353 => {
                let channel = strip_statusmsg(msg.param(2).unwrap_or(""));
                let names = msg.param(3).unwrap_or("");
                if channel.is_empty() || names.is_empty() {
                    return out;
                }
                let buffer = self.channel_buffer(channel);
                let prefixes = self.with_model(|model| {
                    let mut prefixes: Vec<char> = vec!['~', '&', '@', '%', '+'];
                    if let Some(network) = model.network(self.network_id) {
                        prefixes.extend(network.prefix_modes.iter().map(|(_, p)| *p));
                    }
                    prefixes.sort_unstable();
                    prefixes.dedup();
                    prefixes
                });
                self.with_model(|model| {
                    if let Some(buffer) = model.buffer_mut(buffer) {
                        for token in names.split_whitespace() {
                            if let Some(user) = parse_names_user(token, &prefixes) {
                                buffer.add_or_update_user(user);
                            }
                        }
                        buffer.sort_users();
                    }
                });
                self.wake();
            }
            366 => {
                let channel = strip_statusmsg(msg.param(1).unwrap_or(""));
                if let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) {
                    let count = self.with_model(|model| {
                        model
                            .buffer(buffer)
                            .map(|buffer| buffer.user_count())
                            .unwrap_or(0)
                    });
                    self.server_line(
                        LineKind::Info,
                        lmsg!(
                            format!("{count} pseudos dans {channel}."),
                            format!("{count} nicknames in {channel}.")
                        ),
                    );
                }
            }
            372 => {
                let text = msg.param(1).unwrap_or("");
                let text = text.strip_prefix("- ").unwrap_or(text);
                self.server_line(LineKind::Motd, text);
            }
            375 => {
                self.server_line(
                    LineKind::Motd,
                    t("— Message du jour —", "— Message of the day —"),
                );
            }
            376 => {
                self.server_line(
                    LineKind::Motd,
                    t(
                        "— Fin du message du jour —",
                        "— End of message of the day —",
                    ),
                );
            }
            401 => {
                let target = msg.param(1).unwrap_or("?");
                self.server_line(
                    LineKind::Error,
                    lmsg!(
                        format!("{target} : pseudo ou salon introuvable."),
                        format!("{target}: no such nickname or channel.")
                    ),
                );
            }
            403 => {
                let channel = msg.param(1).unwrap_or("?");
                self.error_to_channel(
                    channel,
                    lmsg!(
                        format!("{channel} : salon introuvable."),
                        format!("{channel}: no such channel.")
                    ),
                );
            }
            404 => {
                let channel = msg.param(1).unwrap_or("?");
                self.error_to_channel(
                    channel,
                    lmsg!(
                        format!("Impossible d'écrire sur {channel} : salon muet ?"),
                        format!("Cannot write to {channel}: channel muted?")
                    ),
                );
            }
            421 => {
                let command = msg.param(1).unwrap_or("?");
                if command.eq_ignore_ascii_case("LIST") {
                    self.with_model(|model| model.finish_channel_list(self.network_id));
                    self.wake();
                }
                self.server_line(
                    LineKind::Error,
                    lmsg!(
                        format!("Commande inconnue du serveur : {command}"),
                        format!("Unknown server command: {command}")
                    ),
                );
            }
            433 | 432 => {
                let nick = msg.param(1).unwrap_or(&self.nick).to_string();
                if !self.registered && self.nick_retries < 5 {
                    self.nick_retries += 1;
                    let new_nick = format!("{nick}_");
                    self.nick = new_nick.clone();
                    self.with_model(|model| {
                        model.set_network_nick(self.network_id, new_nick.clone());
                    });
                    self.server_line(
                        LineKind::Error,
                        format!("Pseudo {nick} indisponible, essai avec {new_nick}."),
                    );
                    out.push(format!("NICK {new_nick}"));
                } else {
                    self.server_line(LineKind::Error, format!("Pseudo {nick} indisponible."));
                }
            }
            442 => {
                let channel = msg.param(1).unwrap_or("?");
                self.error_to_channel(
                    channel,
                    lmsg!(
                        format!("Vous n'êtes pas sur {channel}."),
                        format!("You are not on {channel}.")
                    ),
                );
            }
            443 => {
                let channel = msg.param(1).unwrap_or("?");
                self.error_to_channel(
                    channel,
                    lmsg!(
                        format!("Vous êtes déjà sur {channel}."),
                        format!("You are already on {channel}.")
                    ),
                );
            }
            464 => {
                self.server_line(LineKind::Error, "Mot de passe serveur incorrect.");
            }
            471 | 473 | 474 | 475 | 477 => {
                let channel = msg.param(1).unwrap_or("?");
                let reason = msg.param(2).unwrap_or("");
                let text = match numeric {
                    471 => lmsg!(
                        format!("{channel} est complet."),
                        format!("{channel} is full.")
                    ),
                    473 => lmsg!(
                        format!("Impossible de rejoindre {channel} : invitation requise."),
                        format!("Cannot join {channel}: invitation required.")
                    ),
                    474 => lmsg!(
                        format!("Impossible de rejoindre {channel} : vous êtes banni."),
                        format!("Cannot join {channel}: you are banned.")
                    ),
                    475 => lmsg!(
                        format!("Impossible de rejoindre {channel} : clé incorrecte."),
                        format!("Cannot join {channel}: incorrect key.")
                    ),
                    477 => lmsg!(
                        format!("Impossible de rejoindre {channel} : vous devez être enregistré."),
                        format!("Cannot join {channel}: you must be registered.")
                    ),
                    _ => reason.to_string(),
                };
                self.error_to_channel(channel, text);
            }
            482 => {
                let channel = msg.param(1).unwrap_or("?");
                self.error_to_channel(
                    channel,
                    lmsg!(
                        format!("Vous n'êtes pas opérateur sur {channel}."),
                        format!("You are not an operator on {channel}.")
                    ),
                );
            }
            730 | 731 => {
                let nick = msg.param(1).unwrap_or("?");
                let list = msg.param(2).unwrap_or("");
                let online = numeric == 730;
                self.with_model(|model| {
                    if let Some(network) = model.network_mut(self.network_id) {
                        if online {
                            if !network
                                .monitor
                                .iter()
                                .any(|n| irc_fold(n) == irc_fold(nick))
                            {
                                network.monitor.push(nick.to_string());
                            }
                        } else {
                            network.monitor.retain(|n| irc_fold(n) != irc_fold(nick));
                        }
                    }
                    model.touch();
                });
                self.server_line(
                    if online {
                        LineKind::Info
                    } else {
                        LineKind::Away
                    },
                    format!(
                        "{nick} est {} (monitor : {list}).",
                        if online { "en ligne" } else { "hors ligne" }
                    ),
                );
            }
            900 => {
                let text = msg.param(3).unwrap_or("authentification SASL réussie");
                self.server_line(
                    LineKind::Info,
                    lmsg!(format!("SASL : {text}"), format!("SASL: {text}")),
                );
            }
            903 => {
                self.sasl = SaslState::None;
                self.server_line(
                    LineKind::Info,
                    t(
                        "SASL : authentification réussie.",
                        "SASL: authentication successful.",
                    ),
                );
                self.cap_end(&mut out);
            }
            904 | 905 | 906 => {
                self.sasl = SaslState::None;
                self.server_line(
                    LineKind::Error,
                    t(
                        "SASL : échec de l'authentification, connexion sans SASL.",
                        "SASL: authentication failed, connecting without SASL.",
                    ),
                );
                self.cap_end(&mut out);
            }
            907 => {
                self.sasl = SaslState::None;
                self.server_line(
                    LineKind::Info,
                    t("SASL : déjà authentifié.", "SASL: already authenticated."),
                );
                self.cap_end(&mut out);
            }
            908 => {
                let mechanisms = msg.param(1).unwrap_or("PLAIN");
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("SASL : mécanismes disponibles : {mechanisms}"),
                        format!("SASL: available mechanisms: {mechanisms}")
                    ),
                );
            }
            _ => {
                if (400..600).contains(&numeric) {
                    let text = format!(
                        "{} {}",
                        numeric,
                        msg.params
                            .iter()
                            .skip(1)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                    let buffer = if let Some(channel) = msg
                        .param(1)
                        .filter(|value| is_channel_name(strip_statusmsg(value)))
                    {
                        self.channel_buffer(channel)
                    } else {
                        self.server_buffer()
                    };
                    self.append(
                        buffer,
                        LineKind::Error,
                        None,
                        text.trim().to_string(),
                        false,
                        false,
                        msg.server_time(),
                    );
                } else {
                    log::debug!("numeric {numeric} non géré : {:?}", msg.params);
                }
            }
        }
        out
    }

    fn whois_line(&self, text: impl Into<String>) {
        let buffer = self
            .active_buffer_id()
            .filter(|id| {
                self.with_model(|model| {
                    model
                        .buffer(*id)
                        .is_some_and(|buffer| buffer.kind == BufferKind::Query)
                })
            })
            .unwrap_or_else(|| self.server_buffer());
        self.append(buffer, LineKind::Info, None, text, false, false, None);
    }

    fn error_to_channel(&self, channel: &str, text: impl Into<String>) {
        let buffer = self
            .existing_buffer(BufferKind::Channel, channel)
            .unwrap_or_else(|| self.server_buffer());
        self.append(buffer, LineKind::Error, None, text, false, false, None);
    }

    fn on_join(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let channel = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        if from.is_empty() || channel.is_empty() {
            return Vec::new();
        }
        let account = msg
            .param(1)
            .filter(|value| *value != "*")
            .map(str::to_string);
        let realname = msg.param(2).map(str::to_string);
        let buffer = self.channel_buffer(&channel);
        self.with_model(|model| {
            if let Some(buffer) = model.buffer_mut(buffer) {
                buffer.joined = true;
                if irc_fold(&from) == irc_fold(&self.nick) {
                    buffer.users.clear();
                    buffer.typing.clear();
                }
                let mut user = User::new(&from);
                user.account = account.clone();
                user.realname = realname.clone();
                buffer.add_or_update_user(user);
            }
            model.touch();
        });
        if irc_fold(&from) == irc_fold(&self.nick) {
            let first_channel = self.with_model(|model| {
                model
                    .network(self.network_id)
                    .map(|network| {
                        network
                            .buffers
                            .iter()
                            .filter(|id| {
                                model.buffers.get(id).is_some_and(|buffer| {
                                    buffer.kind == BufferKind::Channel && buffer.joined
                                })
                            })
                            .count()
                            == 1
                    })
                    .unwrap_or(false)
            });
            if first_channel {
                self.with_model(|model| model.set_active(buffer));
            }
            self.append(
                buffer,
                LineKind::Join,
                Some(from),
                lmsg!(
                    format!("Vous avez rejoint {channel}."),
                    format!("You joined {channel}.")
                ),
                false,
                true,
                msg.server_time(),
            );
        } else {
            let text = lmsg!(
                if let Some(account) = &account {
                    format!("{from} ({account}) a rejoint {channel}.")
                } else {
                    format!("{from} a rejoint {channel}.")
                },
                if let Some(account) = &account {
                    format!("{from} ({account}) joined {channel}.")
                } else {
                    format!("{from} joined {channel}.")
                }
            );
            self.append(
                buffer,
                LineKind::Join,
                Some(from),
                text,
                false,
                false,
                msg.server_time(),
            );
        }
        Vec::new()
    }

    fn on_part(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let channel = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        let reason = msg.param(1).unwrap_or("").to_string();
        if from.is_empty() || channel.is_empty() {
            return Vec::new();
        }
        let Some(buffer) = self.existing_buffer(BufferKind::Channel, &channel) else {
            return Vec::new();
        };
        self.with_model(|model| {
            if let Some(buffer) = model.buffer_mut(buffer) {
                buffer.remove_user(&from);
                if irc_fold(&from) == irc_fold(&self.nick) {
                    buffer.joined = false;
                    buffer.users.clear();
                    buffer.typing.clear();
                }
            }
            model.touch();
        });
        let text = lmsg!(
            if irc_fold(&from) == irc_fold(&self.nick) {
                format!("Vous avez quitté {channel}{}", reason_suffix(&reason))
            } else {
                format!("{from} a quitté {channel}{}", reason_suffix(&reason))
            },
            if irc_fold(&from) == irc_fold(&self.nick) {
                format!("You left {channel}{}", reason_suffix(&reason))
            } else {
                format!("{from} left {channel}{}", reason_suffix(&reason))
            }
        );
        self.append(
            buffer,
            LineKind::Part,
            Some(from),
            text,
            false,
            false,
            msg.server_time(),
        );
        Vec::new()
    }

    fn on_quit(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        if from.is_empty() {
            return Vec::new();
        }
        let reason = msg.param(0).unwrap_or("").to_string();
        let buffers = self.with_model(|model| model.user_buffers(self.network_id, &from));
        self.with_model(|model| {
            model.remove_user_everywhere(self.network_id, &from);
        });
        let text = lmsg!(
            format!("{from} a quitté{}", reason_suffix(&reason)),
            format!("{from} quit{}", reason_suffix(&reason))
        );
        for buffer in buffers {
            self.append(
                buffer,
                LineKind::Quit,
                Some(from.clone()),
                text.clone(),
                false,
                false,
                msg.server_time(),
            );
        }
        Vec::new()
    }

    fn on_nick(&mut self, msg: &Message) -> Vec<String> {
        let old = msg.nick().unwrap_or("").to_string();
        let new = msg.param(0).unwrap_or("").to_string();
        if old.is_empty() || new.is_empty() {
            return Vec::new();
        }
        let buffers = self.with_model(|model| model.user_buffers(self.network_id, &old));
        self.with_model(|model| {
            model.rename_user_everywhere(self.network_id, &old, &new);
        });
        if irc_fold(&old) == irc_fold(&self.nick) {
            self.nick = new.clone();
            self.server_line(
                LineKind::Info,
                lmsg!(
                    format!("Vous êtes maintenant connu sous le nom de {new}."),
                    format!("You are now known as {new}.")
                ),
            );
        }
        for buffer in buffers {
            self.append(
                buffer,
                LineKind::Nick,
                Some(old.clone()),
                lmsg!(
                    format!("{old} est maintenant {new}."),
                    format!("{old} is now {new}.")
                ),
                false,
                false,
                msg.server_time(),
            );
        }
        Vec::new()
    }

    fn on_privmsg(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let target = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        let raw = msg.param(1).unwrap_or("").to_string();
        if from.is_empty() || target.is_empty() {
            return Vec::new();
        }
        let mut kind = LineKind::Message;
        let mut text = raw.clone();
        if raw.starts_with('\u{01}') && raw.ends_with('\u{01}') && raw.len() >= 2 {
            let inner = &raw[1..raw.len() - 1];
            if let Some(action) = inner.strip_prefix("ACTION ") {
                kind = LineKind::Action;
                text = action.to_string();
            } else if inner == "ACTION" {
                kind = LineKind::Action;
                text = String::new();
            } else {
                return self.reply_ctcp(&from, inner);
            }
        }
        let buffer = if is_channel_name(&target) {
            self.channel_buffer(&target)
        } else {
            self.query_buffer(&from)
        };
        let self_nick = irc_fold(&from) == irc_fold(&self.nick);
        if self_nick && self.echo_message() && self.dedupe_self_echo(buffer, &text) {
            return Vec::new();
        }
        let mention = !self_nick && mentions(&text, &self.nick);
        let kind = self.line_kind(msg, kind);
        self.append(
            buffer,
            kind,
            Some(from),
            text,
            mention,
            self_nick,
            msg.server_time(),
        );
        Vec::new()
    }

    fn reply_ctcp(&mut self, from: &str, payload: &str) -> Vec<String> {
        let (command, rest) = match payload.split_once(' ') {
            Some((command, rest)) => (command.to_ascii_uppercase(), rest),
            None => (payload.to_ascii_uppercase(), ""),
        };
        match command.as_str() {
            "VERSION" => vec![format!(
                "NOTICE {from} :\u{01}VERSION {APP_NAME} {VERSION} (Rust + GPUI)\u{01}"
            )],
            "PING" => vec![format!("NOTICE {from} :\u{01}PING {rest}\u{01}")],
            "TIME" => vec![format!(
                "NOTICE {from} :\u{01}TIME {}\u{01}",
                Local::now().format("%Y-%m-%d %H:%M:%S %Z")
            )],
            "CLIENTINFO" => vec![format!(
                "NOTICE {from} :\u{01}CLIENTINFO ACTION VERSION PING TIME CLIENTINFO\u{01}"
            )],
            _ => Vec::new(),
        }
    }

    fn on_notice(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let target = msg.param(0).unwrap_or("").to_string();
        let text = msg.param(1).unwrap_or("").to_string();
        let prefix_is_server = msg.prefix.as_ref().is_some_and(|prefix| prefix.is_server());
        if prefix_is_server || target == "*" || target.is_empty() {
            self.server_line(LineKind::Notice, text);
            return Vec::new();
        }
        let buffer = if is_channel_name(&target) {
            self.channel_buffer(&target)
        } else {
            self.query_buffer(&from)
        };
        let mention = mentions(&text, &self.nick);
        self.append(
            buffer,
            LineKind::Notice,
            Some(from),
            text,
            mention,
            false,
            msg.server_time(),
        );
        Vec::new()
    }

    fn on_tagmsg(&mut self, msg: &Message) -> Vec<String> {
        let target = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        let Some(from) = msg.nick().map(str::to_string) else {
            return Vec::new();
        };
        let Some(typing) = msg.tag("+typing") else {
            return Vec::new();
        };
        let buffer = if is_channel_name(&target) {
            self.channel_buffer(&target)
        } else {
            self.query_buffer(&from)
        };
        self.with_model(|model| {
            if let Some(buffer) = model.buffer_mut(buffer) {
                if typing == "active" {
                    buffer.set_typing(&from);
                } else {
                    buffer.typing.remove(&irc_fold(&from));
                }
            }
            model.touch();
        });
        self.wake();
        Vec::new()
    }

    fn on_topic(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let channel = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        let topic = msg.param(1).unwrap_or("").to_string();
        if channel.is_empty() {
            return Vec::new();
        }
        let buffer = self.channel_buffer(&channel);
        self.with_model(|model| {
            model.set_topic(buffer, topic.clone(), Some(from.clone()), None);
        });
        self.append(
            buffer,
            LineKind::Topic,
            Some(from.clone()),
            lmsg!(
                format!("{from} a changé le sujet : {topic}"),
                format!("{from} changed the topic: {topic}")
            ),
            false,
            false,
            msg.server_time(),
        );
        Vec::new()
    }

    fn on_mode(&mut self, msg: &Message) -> Vec<String> {
        let target = msg.param(0).unwrap_or("").to_string();
        let modes = msg.param(1).unwrap_or("").to_string();
        if target.is_empty() {
            return Vec::new();
        }
        if is_channel_name(strip_statusmsg(&target)) {
            self.on_channel_mode(msg, strip_statusmsg(&target), &modes);
        } else {
            let from = msg.nick().unwrap_or("serveur").to_string();
            self.server_line(
                LineKind::Mode,
                lmsg!(
                    format!("{from} règle les modes de {target} : {modes}"),
                    format!("{from} sets modes on {target}: {modes}")
                ),
            );
            if irc_fold(&target) == irc_fold(&self.nick) && modes.contains('i') {
                // Rien de spécial : le mode +i est simplement affiché.
            }
        }
        Vec::new()
    }

    fn on_channel_mode(&mut self, msg: &Message, channel: &str, modes: &str) {
        let Some(buffer) = self.existing_buffer(BufferKind::Channel, channel) else {
            return;
        };
        let prefix_modes = self.with_model(|model| {
            model
                .network(self.network_id)
                .map(|network| network.prefix_modes.clone())
                .unwrap_or_else(|| vec![('o', '@'), ('v', '+')])
        });
        let params: Vec<String> = msg.params.iter().skip(2).cloned().collect();
        let mut changes: Vec<(char, char, String)> = Vec::new();
        self.with_model(|model| {
            if let Some(buffer) = model.buffer_mut(buffer) {
                let mut sign = '+';
                let mut param_index = 0usize;
                for mode in modes.chars() {
                    match mode {
                        '+' | '-' => {
                            sign = mode;
                            continue;
                        }
                        _ => {}
                    }
                    if let Some((_, prefix)) = prefix_modes
                        .iter()
                        .find(|(mode_char, _)| *mode_char == mode)
                    {
                        if let Some(nick) = params.get(param_index).cloned() {
                            param_index += 1;
                            if let Some(user) = buffer.user_mut(&nick) {
                                if sign == '+' {
                                    user.add_prefix(*prefix);
                                } else {
                                    user.remove_prefix(*prefix);
                                }
                            }
                            changes.push((sign, *prefix, nick));
                        }
                    } else if matches!(mode, 'b' | 'e' | 'I' | 'k') {
                        param_index += 1;
                    } else if mode == 'l' && sign == '+' {
                        param_index += 1;
                    }
                }
                update_mode_string(&mut buffer.modes, modes);
            }
            model.touch();
        });
        let from = msg.nick().unwrap_or("serveur").to_string();
        let mut details = modes.to_string();
        if !params.is_empty() {
            details.push(' ');
            details.push_str(&params.join(" "));
        }
        self.append(
            buffer,
            LineKind::Mode,
            Some(from.clone()),
            lmsg!(
                format!("{from} règle les modes de {channel} : {details}"),
                format!("{from} sets modes on {channel}: {details}")
            ),
            false,
            false,
            msg.server_time(),
        );
        for (sign, prefix, nick) in changes {
            let text = lmsg!(
                if sign == '+' {
                    format!("{nick} reçoit le préfixe {prefix}")
                } else {
                    format!("{nick} perd le préfixe {prefix}")
                },
                if sign == '+' {
                    format!("{nick} gained prefix {prefix}")
                } else {
                    format!("{nick} lost prefix {prefix}")
                }
            );
            self.append(
                buffer,
                LineKind::Mode,
                Some(nick),
                text,
                false,
                false,
                msg.server_time(),
            );
        }
    }

    fn on_kick(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let channel = strip_statusmsg(msg.param(0).unwrap_or("")).to_string();
        let victim = msg.param(1).unwrap_or("").to_string();
        let reason = msg.param(2).unwrap_or("").to_string();
        let Some(buffer) = self.existing_buffer(BufferKind::Channel, &channel) else {
            return Vec::new();
        };
        self.with_model(|model| {
            if let Some(buffer) = model.buffer_mut(buffer) {
                buffer.remove_user(&victim);
                if irc_fold(&victim) == irc_fold(&self.nick) {
                    buffer.joined = false;
                    buffer.users.clear();
                    buffer.typing.clear();
                }
            }
            model.touch();
        });
        let text = lmsg!(
            if irc_fold(&victim) == irc_fold(&self.nick) {
                format!(
                    "Vous avez été expulsé de {channel} par {from}{}",
                    reason_suffix(&reason)
                )
            } else {
                format!(
                    "{victim} a été expulsé de {channel} par {from}{}",
                    reason_suffix(&reason)
                )
            },
            if irc_fold(&victim) == irc_fold(&self.nick) {
                format!(
                    "You were kicked from {channel} by {from}{}",
                    reason_suffix(&reason)
                )
            } else {
                format!(
                    "{victim} was kicked from {channel} by {from}{}",
                    reason_suffix(&reason)
                )
            }
        );
        self.append(
            buffer,
            LineKind::Kick,
            Some(from),
            text,
            false,
            false,
            msg.server_time(),
        );
        Vec::new()
    }

    fn on_invite(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let target = msg.param(0).unwrap_or("").to_string();
        let channel = msg.param(1).unwrap_or("").to_string();
        if irc_fold(&target) == irc_fold(&self.nick) {
            let buffer = self.active_or_server();
            self.append(
                buffer,
                LineKind::Invite,
                Some(from.clone()),
                lmsg!(
                    format!("{from} vous invite sur {channel} (/join {channel})."),
                    format!("{from} invited you to {channel} (/join {channel}).")
                ),
                false,
                false,
                msg.server_time(),
            );
        }
        Vec::new()
    }

    fn on_away(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let reason = msg.param(0).unwrap_or("").to_string();
        if from.is_empty() {
            return Vec::new();
        }
        let affected = self.with_model(|model| model.user_buffers(self.network_id, &from));
        let away = !reason.is_empty();
        self.with_model(|model| {
            model.for_user_buffers(self.network_id, &from, |user| user.away = away);
        });
        if let Some(active) = self.active_buffer_id() {
            if affected.contains(&active) {
                let text = lmsg!(
                    if away {
                        format!("{from} est maintenant absent : {reason}")
                    } else {
                        format!("{from} n'est plus absent.")
                    },
                    if away {
                        format!("{from} is now away: {reason}")
                    } else {
                        format!("{from} is no longer away.")
                    }
                );
                self.append(
                    active,
                    LineKind::Away,
                    Some(from),
                    text,
                    false,
                    false,
                    msg.server_time(),
                );
            }
        }
        Vec::new()
    }

    fn on_account(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let account = msg.param(0).unwrap_or("").to_string();
        if from.is_empty() {
            return Vec::new();
        }
        let value = (account != "*").then(|| account.clone());
        self.with_model(|model| {
            model.for_user_buffers(self.network_id, &from, |user| {
                user.account = value.clone();
            });
        });
        if let Some(active) = self.active_buffer_id() {
            if self.with_model(|model| {
                model
                    .buffer(active)
                    .is_some_and(|buffer| buffer.user(&from).is_some())
            }) {
                let text = lmsg!(
                    if account == "*" {
                        format!("{from} s'est déconnecté de son compte.")
                    } else {
                        format!("{from} est connecté au compte {account}.")
                    },
                    if account == "*" {
                        format!("{from} logged out of their account.")
                    } else {
                        format!("{from} is logged in as {account}.")
                    }
                );
                self.append(
                    active,
                    LineKind::Account,
                    Some(from),
                    text,
                    false,
                    false,
                    msg.server_time(),
                );
            }
        }
        Vec::new()
    }

    fn on_chghost(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let user = msg.param(0).map(str::to_string);
        let host = msg.param(1).map(str::to_string);
        if from.is_empty() {
            return Vec::new();
        }
        self.with_model(|model| {
            model.for_user_buffers(self.network_id, &from, |entry| {
                entry.user = user.clone();
                entry.host = host.clone();
            });
        });
        if let Some(active) = self.active_buffer_id() {
            if self.with_model(|model| {
                model
                    .buffer(active)
                    .is_some_and(|buffer| buffer.user(&from).is_some())
            }) {
                self.append(
                    active,
                    LineKind::Account,
                    Some(from.clone()),
                    lmsg!(
                        format!(
                            "{from} a changé d'hôte : {}@{}",
                            user.as_deref().unwrap_or(""),
                            host.as_deref().unwrap_or("")
                        ),
                        format!(
                            "{from} changed host: {}@{}",
                            user.as_deref().unwrap_or(""),
                            host.as_deref().unwrap_or("")
                        )
                    ),
                    false,
                    false,
                    msg.server_time(),
                );
            }
        }
        Vec::new()
    }

    fn on_setname(&mut self, msg: &Message) -> Vec<String> {
        let from = msg.nick().unwrap_or("").to_string();
        let realname = msg.param(0).unwrap_or("").to_string();
        if from.is_empty() {
            return Vec::new();
        }
        self.with_model(|model| {
            model.for_user_buffers(self.network_id, &from, |user| {
                user.realname = Some(realname.clone());
            });
        });
        Vec::new()
    }

    fn on_batch(&mut self, msg: &Message) -> Vec<String> {
        let id = msg.param(0).unwrap_or("").to_string();
        if id.is_empty() {
            return Vec::new();
        }
        if let Some(kind) = id.strip_prefix('+') {
            let batch_kind = msg.param(1).unwrap_or("").to_string();
            self.batches.push(Batch {
                id: kind.to_string(),
                kind: batch_kind,
            });
        } else if let Some(kind) = id.strip_prefix('-') {
            self.batches.retain(|batch| batch.id != kind);
        }
        Vec::new()
    }

    fn on_fail(&mut self, msg: &Message, kind: LineKind, label: &str) -> Vec<String> {
        let command = msg.param(0).unwrap_or("");
        let code = msg.param(1).unwrap_or("");
        let context = msg.param(2).unwrap_or("");
        let description = msg.param(3).unwrap_or("");
        self.server_line(
            kind,
            format!("{label} {command} {code} {context} : {description}"),
        );
        Vec::new()
    }

    fn dedupe_self_echo(&self, buffer: BufferId, text: &str) -> bool {
        self.with_model(|model| {
            model.buffer(buffer).is_some_and(|buffer| {
                buffer.messages.back().is_some_and(|line| {
                    line.self_nick
                        && line.text == text
                        && Local::now()
                            .signed_duration_since(line.at)
                            .num_seconds()
                            .abs()
                            <= 3
                })
            })
        })
    }

    fn local_echo(&self, target: &str, kind: LineKind, text: &str, force: bool) {
        if !force && self.echo_message() {
            return;
        }
        let buffer = if is_channel_name(target) {
            self.channel_buffer(target)
        } else {
            self.query_buffer(target)
        };
        self.append(
            buffer,
            kind,
            Some(self.nick.clone()),
            text,
            false,
            true,
            None,
        );
    }

    pub fn handle_user_input(&mut self, input: &str) -> UserOutcome {
        let mut outcome = UserOutcome::default();
        let active = self.with_model(|model| {
            model
                .active_buffer()
                .filter(|buffer| buffer.network == self.network_id)
                .map(|buffer| ActiveBuffer {
                    kind: buffer.kind,
                    name: buffer.name.clone(),
                })
        });
        let parsed = commands::parse(input, active.as_ref(), &self.nick);
        match parsed {
            ParsedCommand::Say { target, text } => {
                if text.is_empty() {
                    return outcome;
                }
                if is_channel_name(&target) && self.caps_acked.contains("typing") {
                    outcome.lines.push(format!("@+typing=done TAGMSG {target}"));
                }
                outcome.lines.push(format!("PRIVMSG {target} :{text}"));
                self.local_echo(&target, LineKind::Message, &text, false);
            }
            ParsedCommand::Action { target, text } => {
                outcome
                    .lines
                    .push(format!("PRIVMSG {target} :\u{01}ACTION {text}\u{01}"));
                self.local_echo(&target, LineKind::Action, &text, false);
            }
            ParsedCommand::Notice { target, text } => {
                outcome.lines.push(format!("NOTICE {target} :{text}"));
                self.local_echo(&target, LineKind::Notice, &text, true);
            }
            ParsedCommand::Ctcp { target, payload } => {
                outcome
                    .lines
                    .push(format!("PRIVMSG {target} :\u{01}{payload}\u{01}"));
            }
            ParsedCommand::Join { channels, keys } => {
                let line = match keys {
                    Some(keys) if !keys.is_empty() => format!("JOIN {channels} {keys}"),
                    _ => format!("JOIN {channels}"),
                };
                outcome.lines.push(line);
                self.server_line(
                    LineKind::Info,
                    format!("Tentative de rejoindre {channels}…"),
                );
            }
            ParsedCommand::Part { channels, reason } => {
                let line = match reason {
                    Some(reason) if !reason.is_empty() => {
                        format!("PART {channels} :{reason}")
                    }
                    _ => format!("PART {channels}"),
                };
                outcome.lines.push(line);
            }
            ParsedCommand::Cycle { channel } => {
                outcome.lines.push(format!("PART {channel} :cycle"));
                outcome.lines.push(format!("JOIN {channel}"));
            }
            ParsedCommand::Nick(nick) => {
                outcome.lines.push(format!("NICK {nick}"));
            }
            ParsedCommand::Query { target, text } => {
                let buffer = self.query_buffer(&target);
                self.with_model(|model| model.set_active(buffer));
                self.wake();
                if let Some(text) = text {
                    if !text.is_empty() {
                        if self.caps_acked.contains("typing") {
                            outcome.lines.push(format!("@+typing=done TAGMSG {target}"));
                        }
                        outcome.lines.push(format!("PRIVMSG {target} :{text}"));
                        self.local_echo(&target, LineKind::Message, &text, false);
                    }
                }
            }
            ParsedCommand::Topic { channel, text } => {
                let channel = channel.or_else(|| {
                    active
                        .as_ref()
                        .and_then(ActiveBuffer::target)
                        .map(str::to_string)
                });
                let Some(channel) = channel else {
                    self.local_error(t(
                        "Aucun salon actif pour /topic.",
                        "No active channel for /topic.",
                    ));
                    return outcome;
                };
                match text {
                    Some(text) if !text.is_empty() => {
                        outcome.lines.push(format!("TOPIC {channel} :{text}"));
                    }
                    _ => outcome.lines.push(format!("TOPIC {channel}")),
                }
            }
            ParsedCommand::Names(channel) => {
                outcome.lines.push(match channel {
                    Some(channel) => format!("NAMES {channel}"),
                    None => "NAMES".to_string(),
                });
            }
            ParsedCommand::Whois(nick) => {
                outcome.lines.push(format!("WHOIS {nick}"));
            }
            ParsedCommand::Who(mask) => {
                outcome.lines.push(match mask {
                    Some(mask) => format!("WHO {mask}"),
                    None => "WHO".to_string(),
                });
            }
            ParsedCommand::Whowas(nick) => {
                outcome.lines.push(format!("WHOWAS {nick}"));
            }
            ParsedCommand::List(filter) => {
                outcome.lines.push(match filter {
                    Some(filter) => format!("LIST {filter}"),
                    None => "LIST".to_string(),
                });
            }
            ParsedCommand::Mode { target, modes } => {
                if modes.is_empty() {
                    outcome.lines.push(format!("MODE {target}"));
                } else {
                    outcome.lines.push(format!("MODE {target} {modes}"));
                }
            }
            ParsedCommand::UserMode {
                channel,
                mode,
                add,
                nicks,
            } => {
                let Some(channel) = channel else {
                    self.local_error(t(
                        "Aucun salon actif pour cette commande de mode.",
                        "No active channel for this mode command.",
                    ));
                    return outcome;
                };
                let sign = if add { '+' } else { '-' };
                for chunk in nicks.chunks(4) {
                    let modes = format!("{sign}{}", mode.to_string().repeat(chunk.len()));
                    outcome
                        .lines
                        .push(format!("MODE {channel} {modes} {}", chunk.join(" ")));
                }
            }
            ParsedCommand::Kick {
                channel,
                nick,
                reason,
            } => {
                let Some(channel) = channel else {
                    self.local_error(t(
                        "Aucun salon actif pour /kick.",
                        "No active channel for /kick.",
                    ));
                    return outcome;
                };
                let line = match reason {
                    Some(reason) if !reason.is_empty() => {
                        format!("KICK {channel} {nick} :{reason}")
                    }
                    _ => format!("KICK {channel} {nick}"),
                };
                outcome.lines.push(line);
            }
            ParsedCommand::Ban {
                channel,
                mask,
                remove,
            } => {
                let Some(channel) = channel else {
                    self.local_error(t(
                        "Aucun salon actif pour /ban.",
                        "No active channel for /ban.",
                    ));
                    return outcome;
                };
                let sign = if remove { '-' } else { '+' };
                outcome.lines.push(format!("MODE {channel} {sign}b {mask}"));
            }
            ParsedCommand::Invite { nick, channel } => {
                let channel = channel.or_else(|| {
                    active
                        .as_ref()
                        .and_then(ActiveBuffer::target)
                        .map(str::to_string)
                });
                outcome.lines.push(match channel {
                    Some(channel) => format!("INVITE {nick} {channel}"),
                    None => format!("INVITE {nick}"),
                });
            }
            ParsedCommand::Away(message) => {
                outcome.lines.push(match message {
                    Some(message) if !message.is_empty() => format!("AWAY :{message}"),
                    _ => "AWAY".to_string(),
                });
            }
            ParsedCommand::Monitor(value) => {
                outcome.lines.push(format!("MONITOR {value}"));
            }
            ParsedCommand::Raw(line) => {
                if line.to_ascii_uppercase().starts_with("QUIT") {
                    outcome.action = ConnectionAction::Disconnect;
                }
                outcome.lines.push(line);
            }
            ParsedCommand::SetName(name) => {
                outcome.lines.push(format!("SETNAME :{name}"));
            }
            ParsedCommand::Ping(token) => {
                let token = token.unwrap_or_else(|| {
                    format!("{}-{}", self.config.nick, Local::now().timestamp())
                });
                self.ping_token = Some(token.clone());
                self.ping_sent_at = Some(Instant::now());
                outcome.lines.push(format!("PING :{token}"));
                self.server_line(
                    LineKind::Info,
                    lmsg!(
                        format!("PING envoyé ({token})."),
                        format!("PING sent ({token}).")
                    ),
                );
            }
            ParsedCommand::Oper { name, password } => {
                outcome.lines.push(format!("OPER {name} {password}"));
            }
            ParsedCommand::Wallops(text) => {
                outcome.lines.push(format!("WALLOPS :{text}"));
            }
            ParsedCommand::Local(command) => {
                self.handle_local(command, &mut outcome);
            }
        }
        outcome
    }

    fn handle_local(&mut self, command: LocalCommand, outcome: &mut UserOutcome) {
        match command {
            LocalCommand::Clear => {
                if let Some(buffer) = self.active_buffer_id() {
                    self.with_model(|model| model.clear_buffer(buffer));
                }
            }
            LocalCommand::Close => {
                let Some(buffer) = self.active_buffer_id() else {
                    return;
                };
                let info = self.with_model(|model| {
                    model
                        .buffer(buffer)
                        .map(|buffer| (buffer.kind, buffer.name.clone()))
                });
                match info {
                    Some((BufferKind::Channel, name)) => {
                        outcome.lines.push(format!("PART {name} :fermeture"));
                        self.with_model(|model| model.close_buffer(buffer));
                        self.wake();
                    }
                    Some((BufferKind::Query, _)) => {
                        self.with_model(|model| model.close_buffer(buffer));
                        self.wake();
                    }
                    Some((BufferKind::Server, _)) => {
                        self.local_error("Utilisez /quit pour quitter le réseau.");
                    }
                    None => {}
                }
            }
            LocalCommand::Help(topic) => {
                let buffer = self.active_or_server();
                if let Some(topic) = topic {
                    self.append(
                        buffer,
                        LineKind::Info,
                        None,
                        lmsg!(
                            format!("Aide : /{topic} — voir la liste ci-dessous."),
                            format!("Help: /{topic} — see the list below.")
                        ),
                        false,
                        false,
                        None,
                    );
                }
                for line in commands::help_lines() {
                    self.append(
                        buffer,
                        LineKind::Info,
                        None,
                        line.to_string(),
                        false,
                        false,
                        None,
                    );
                }
            }
            LocalCommand::Version => {
                self.server_line(
                    LineKind::Info,
                    format!("{APP_NAME} {VERSION} — Rust + GPUI, noyau ircv3-core."),
                );
            }
            LocalCommand::Time => {
                self.server_line(
                    LineKind::Info,
                    format!(
                        "Heure locale : {}",
                        Local::now().format("%Y-%m-%d %H:%M:%S %Z")
                    ),
                );
            }
            LocalCommand::Quit(reason) => {
                let line = match reason {
                    Some(reason) if !reason.is_empty() => format!("QUIT :{reason}"),
                    _ => "QUIT :à bientôt".to_string(),
                };
                outcome.lines.push(line);
                outcome.action = ConnectionAction::Disconnect;
            }
            LocalCommand::Reconnect => {
                outcome.action = ConnectionAction::Reconnect;
            }
            LocalCommand::Disconnect => {
                outcome.action = ConnectionAction::Disconnect;
            }
            LocalCommand::PartAll => {
                outcome.lines.push("JOIN 0".to_string());
            }
            LocalCommand::Error(message) => {
                self.local_error(message);
            }
            LocalCommand::Info(message) => {
                if !message.is_empty() {
                    self.server_line(LineKind::Info, message);
                }
            }
        }
    }

    pub fn send_privmsg(&self, target: &str, text: &str) -> Vec<String> {
        if target.is_empty() || text.is_empty() {
            return Vec::new();
        }
        let target = strip_statusmsg(target);
        if !self.echo_message() {
            let buffer = if is_channel_name(target) {
                self.channel_buffer(target)
            } else {
                self.query_buffer(target)
            };
            self.append(
                buffer,
                LineKind::Message,
                Some(self.nick.clone()),
                text.to_string(),
                false,
                true,
                None,
            );
        }
        vec![format!("PRIVMSG {target} :{text}")]
    }

    pub fn local_note(&self, kind: LineKind, text: impl Into<String>) {
        let buffer = self.active_or_server();
        self.append(buffer, kind, None, text, false, false, None);
    }

    pub fn start_dcc(&self, nick: &str, path: &std::path::Path) -> Result<Vec<String>, String> {
        let payload = crate::dcc::start_send(
            self.network_id,
            self.model.clone(),
            self.wake.clone(),
            nick,
            path,
        )?;
        Ok(vec![format!("PRIVMSG {nick} :\u{01}{payload}\u{01}")])
    }

    pub fn cancel_dcc(&self, id: Option<u64>) {
        self.with_model(|model| model.cancel_dcc(id));
        self.wake();
    }

    pub fn local_error(&self, message: impl Into<String>) {
        let buffer = self.active_or_server();
        self.append(buffer, LineKind::Error, None, message, false, false, None);
    }
}

fn parse_prefix(value: &str) -> Vec<(char, char)> {
    let Some(open) = value.find('(') else {
        return vec![('o', '@'), ('v', '+')];
    };
    let Some(close) = value.find(')') else {
        return vec![('o', '@'), ('v', '+')];
    };
    if close <= open {
        return vec![('o', '@'), ('v', '+')];
    }
    let modes: Vec<char> = value[open + 1..close].chars().collect();
    let prefixes: Vec<char> = value[close + 1..].chars().collect();
    modes
        .into_iter()
        .zip(prefixes)
        .collect::<Vec<_>>()
        .into_iter()
        .collect()
}

fn parse_names_user(token: &str, prefixes: &[char]) -> Option<User> {
    if token.is_empty() {
        return None;
    }
    let mut rest = token;
    let mut user_prefixes = String::new();
    loop {
        let Some(first) = rest.chars().next() else {
            break;
        };
        if prefixes.contains(&first) && rest.len() > first.len_utf8() {
            user_prefixes.push(first);
            rest = &rest[first.len_utf8()..];
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return None;
    }
    let (nick, user_name, host) = match rest.split_once('!') {
        Some((nick, rest)) => {
            let (user_name, host) = match rest.split_once('@') {
                Some((user_name, host)) => (Some(user_name.to_string()), Some(host.to_string())),
                None => (Some(rest.to_string()), None),
            };
            (nick.to_string(), user_name, host)
        }
        None => (rest.to_string(), None, None),
    };
    let mut user = User::new(nick);
    user.prefixes = user_prefixes;
    user.user = user_name;
    user.host = host;
    Some(user)
}

fn update_mode_string(current: &mut String, changes: &str) {
    let mut modes: std::collections::BTreeSet<char> = current.chars().collect();
    let mut sign = '+';
    for c in changes.chars() {
        match c {
            '+' | '-' => sign = c,
            _ if sign == '+' => {
                modes.insert(c);
            }
            _ => {
                modes.remove(&c);
            }
        }
    }
    *current = modes.into_iter().collect();
}

fn trailing_after(msg: &Message, index: usize) -> String {
    msg.params
        .iter()
        .skip(index)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

fn reason_suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(" ({reason})")
    }
}

fn strip_statusmsg(value: &str) -> &str {
    let mut value = value;
    loop {
        let Some(first) = value.chars().next() else {
            break;
        };
        if matches!(first, '@' | '+') {
            let rest = &value[first.len_utf8()..];
            if rest.starts_with(['#', '&', '+', '!']) {
                value = rest;
                continue;
            }
        }
        break;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, model::AppModel};
    use std::sync::Arc;

    fn session() -> (Session, Arc<Mutex<AppModel>>) {
        let model = Arc::new(Mutex::new(AppModel::new(Config::default())));
        let config = ConnectionConfig {
            host: "irc.test".into(),
            port: 6667,
            tls: false,
            verify_tls: false,
            nick: "moi".into(),
            user: "moi".into(),
            realname: "Moi".into(),
            sasl_account: None,
            sasl_password: None,
            channels: vec!["#test".into()],
            auto_reconnect: false,
        };
        let network_id = model.lock().unwrap().add_network(config.clone());
        let (wake, _rx) = async_channel::bounded(1);
        let session = Session::new(network_id, config, model.clone(), wake);
        (session, model)
    }

    #[test]
    fn parses_names_with_prefixes() {
        let prefixes = vec!['@', '+'];
        let user = parse_names_user("@+alice!a@h", &prefixes).unwrap();
        assert_eq!(user.nick, "alice");
        assert_eq!(user.prefixes, "@+");
        assert_eq!(user.user.as_deref(), Some("a"));
        assert_eq!(user.host.as_deref(), Some("h"));
    }

    #[test]
    fn mode_string_updates() {
        let mut modes = String::from("nt");
        update_mode_string(&mut modes, "+o-v+k");
        assert!(modes.contains('n'));
        assert!(modes.contains('t'));
        assert!(modes.contains('k'));
        assert!(!modes.contains('v'));
    }

    #[test]
    fn cap_negotiation_and_welcome() {
        let (mut session, model) = session();
        let out = session.initial_lines();
        assert!(out.contains(&"CAP LS 302".to_string()));
        let caps = session.on_line(
            ":irc.test CAP * LS :message-tags server-time echo-message multi-prefix sasl=PLAIN cap-notify",
        );
        assert!(caps.iter().any(|line| line.starts_with("CAP REQ :")));
        let replies = session.on_line(
            ":irc.test CAP * ACK :message-tags server-time echo-message multi-prefix cap-notify",
        );
        assert!(replies.contains(&"CAP END".to_string()));
        let welcome = session.on_line(":irc.test 001 moi :Bienvenue");
        assert!(welcome.iter().any(|line| line == "JOIN #test"));
        let model = model.lock().unwrap();
        assert_eq!(
            model.active_network().unwrap().status,
            ConnectionStatus::Connected
        );
    }

    #[test]
    fn join_names_and_message() {
        let (mut session, model) = session();
        session.on_line(":moi!m@h JOIN #test");
        session.on_line(":irc.test 353 moi = #test :@moi +alice bob");
        session.on_line(":irc.test 366 moi #test :End");
        session.on_line(":alice!a@h PRIVMSG #test :salut moi");
        let model = model.lock().unwrap();
        let network = model.active_network().unwrap();
        let channel = model
            .find_buffer(network.id, BufferKind::Channel, "#test")
            .unwrap();
        let buffer = model.buffer(channel).unwrap();
        assert_eq!(buffer.users.len(), 3);
        assert!(buffer.user("alice").is_some());
        assert!(buffer.messages.back().unwrap().text.contains("salut"));
        assert!(buffer.messages.back().unwrap().mention);
    }

    #[test]
    fn mode_updates_prefix() {
        let (mut session, model) = session();
        session.on_line(":moi!m@h JOIN #test");
        session.on_line(":irc.test 353 moi = #test :moi alice");
        session.on_line(":alice!a@h MODE #test +o alice");
        let model = model.lock().unwrap();
        let network = model.active_network().unwrap();
        let channel = model
            .find_buffer(network.id, BufferKind::Channel, "#test")
            .unwrap();
        assert_eq!(
            model
                .buffer(channel)
                .unwrap()
                .user("alice")
                .unwrap()
                .prefixes,
            "@"
        );
    }

    #[test]
    fn user_command_say_echoes_locally_without_cap() {
        let (mut session, model) = session();
        session.on_line(":moi!m@h JOIN #test");
        let outcome = session.handle_user_input("bonjour");
        assert_eq!(outcome.lines, vec!["PRIVMSG #test :bonjour"]);
        let model = model.lock().unwrap();
        let network = model.active_network().unwrap();
        let channel = model
            .find_buffer(network.id, BufferKind::Channel, "#test")
            .unwrap();
        let buffer = model.buffer(channel).unwrap();
        assert!(buffer.messages.back().unwrap().self_nick);
        assert_eq!(buffer.messages.back().unwrap().text, "bonjour");
    }

    #[test]
    fn sasl_payload_is_sent() {
        let model = Arc::new(Mutex::new(AppModel::new(Config::default())));
        let config = ConnectionConfig {
            host: "irc.test".into(),
            port: 6667,
            tls: false,
            verify_tls: false,
            nick: "moi".into(),
            user: "moi".into(),
            realname: "Moi".into(),
            sasl_account: Some("compte".into()),
            sasl_password: Some("secret".into()),
            channels: vec![],
            auto_reconnect: false,
        };
        let network_id = model.lock().unwrap().add_network(config.clone());
        let (wake, _rx) = async_channel::bounded(1);
        let mut session = Session::new(network_id, config, model, wake);
        session.on_line(":irc.test CAP * LS :sasl=PLAIN");
        let out = session.on_line(":irc.test CAP * ACK :sasl");
        assert!(out.contains(&"AUTHENTICATE PLAIN".to_string()));
        let replies = session.on_line("AUTHENTICATE +");
        assert!(replies.iter().any(|line| line.starts_with("AUTHENTICATE ")));
        assert!(
            replies
                .iter()
                .any(|line| line.len() > "AUTHENTICATE ".len())
        );
    }
}
