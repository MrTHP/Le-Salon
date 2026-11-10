//! Transport réseau : TCP, TLS (rustls), boucle de lecture et reconnexion.
//!
//! Chaque réseau tourne dans son propre thread. L'interface envoie du texte
//! via un canal ; le thread met à jour le modèle partagé et réveille la vue.

use std::{
    io::{self, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, RootCertStore, SignatureScheme,
    StreamOwned,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};

use crate::{
    Wake,
    config::ConnectionConfig,
    model::{AppModel, ConnectionStatus, LineKind, NetworkId},
    protocol::{ConnectionAction, Session},
    truncate_irc_line,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
const READ_POLL: Duration = Duration::from_millis(100);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const RECONNECT_MAX_ATTEMPTS: u32 = 8;

enum ThreadCommand {
    Input(String),
    Raw(String),
    Privmsg { target: String, text: String },
    Local { kind: LineKind, text: String },
    DccSend { nick: String, path: PathBuf },
    DccCancel(Option<u64>),
    Reconnect,
    ReconnectWith(ConnectionConfig),
    Disconnect,
    Shutdown,
}

/// Handle clonable et `Send` pour envoyer des lignes depuis un thread
/// auxiliaire (téléversement, planification, etc.).
#[derive(Clone)]
pub struct LineSender {
    tx: mpsc::Sender<ThreadCommand>,
}

impl LineSender {
    pub fn send_input(&self, input: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Input(input.into()));
    }

    pub fn send_local(&self, kind: LineKind, text: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Local {
            kind,
            text: text.into(),
        });
    }

    pub fn send_message(&self, target: impl Into<String>, text: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Privmsg {
            target: target.into(),
            text: text.into(),
        });
    }
}

pub struct NetworkHandle {
    pub id: NetworkId,
    tx: mpsc::Sender<ThreadCommand>,
    thread: Option<JoinHandle<()>>,
}

impl NetworkHandle {
    pub fn send_input(&self, input: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Input(input.into()));
    }

    pub fn send_raw(&self, line: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Raw(line.into()));
    }

    pub fn send_local(&self, kind: LineKind, text: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Local {
            kind,
            text: text.into(),
        });
    }

    pub fn send_message(&self, target: impl Into<String>, text: impl Into<String>) {
        let _ = self.tx.send(ThreadCommand::Privmsg {
            target: target.into(),
            text: text.into(),
        });
    }

    pub fn dcc_send(&self, nick: impl Into<String>, path: PathBuf) {
        let _ = self.tx.send(ThreadCommand::DccSend {
            nick: nick.into(),
            path,
        });
    }

    pub fn dcc_cancel(&self, id: Option<u64>) {
        let _ = self.tx.send(ThreadCommand::DccCancel(id));
    }

    pub fn line_sender(&self) -> LineSender {
        LineSender {
            tx: self.tx.clone(),
        }
    }

    pub fn reconnect(&self) {
        let _ = self.tx.send(ThreadCommand::Reconnect);
    }

    pub fn reconnect_with(&self, config: ConnectionConfig) {
        let _ = self.tx.send(ThreadCommand::ReconnectWith(config));
    }

    pub fn disconnect(&self) {
        let _ = self.tx.send(ThreadCommand::Disconnect);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(ThreadCommand::Shutdown);
    }
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(ThreadCommand::Shutdown);
        // On ne joint pas le thread ici : la fermeture de l'application ne doit
        // jamais bloquer l'interface. Le thread se termine au prochain tick.
        let _ = self.thread.take();
    }
}

pub fn spawn(
    network_id: NetworkId,
    config: ConnectionConfig,
    model: Arc<Mutex<AppModel>>,
    wake: Wake,
) -> NetworkHandle {
    let (tx, rx) = mpsc::channel();
    let thread_config = config.clone();
    let thread = std::thread::Builder::new()
        .name(format!("ircv3-net-{network_id}"))
        .spawn(move || run(network_id, thread_config, model, wake, rx))
        .expect("impossible de démarrer le thread réseau");
    NetworkHandle {
        id: network_id,
        tx,
        thread: Some(thread),
    }
}

fn run(
    network_id: NetworkId,
    mut config: ConnectionConfig,
    model: Arc<Mutex<AppModel>>,
    wake: Wake,
    rx: mpsc::Receiver<ThreadCommand>,
) {
    let mut session = Session::new(network_id, config.clone(), model, wake);

    // Dernière barrière avant le réseau : même un appel direct à `spawn`
    // (exemple `probe`, test, futur client) ne peut pas envoyer un mot de
    // passe SASL dans un canal non sûr.
    if let Err(error) = config.security_check() {
        session.set_status(ConnectionStatus::Disconnected, error.clone());
        session.local_error(error);
        return;
    }

    let mut should_connect = true;
    let mut auto_reconnect = config.auto_reconnect;
    let mut attempts = 0u32;

    loop {
        if should_connect {
            match connect_and_run(&mut session, &config, &rx) {
                EndReason::Shutdown => break,
                EndReason::Reconnect => {
                    session.reset_for_reconnect();
                    attempts = 0;
                    std::thread::sleep(Duration::from_millis(500));
                }
                EndReason::ReconnectWith(new_config) => {
                    config = new_config;
                    session.config = config.clone();
                    session.reset_for_reconnect();
                    attempts = 0;
                    std::thread::sleep(Duration::from_millis(500));
                }
                EndReason::UserDisconnect(reason) => {
                    session.on_disconnected(reason);
                    should_connect = false;
                    auto_reconnect = false;
                }
                EndReason::Failed { message, retryable } => {
                    session.on_disconnected(Some(message));
                    if retryable && auto_reconnect && attempts < RECONNECT_MAX_ATTEMPTS {
                        if session.ever_registered {
                            attempts = 0;
                        }
                        attempts += 1;
                        let seconds = (2u64 << (attempts - 1).min(5)).min(60);
                        session.reset_for_reconnect();
                        session.set_status(
                            ConnectionStatus::Disconnected,
                            if crate::i18n::lang() == crate::i18n::Lang::En {
                                format!("reconnecting in {seconds}s…")
                            } else {
                                format!("reconnexion dans {seconds} s…")
                            },
                        );
                        match wait_for_command(&rx, Duration::from_secs(seconds), &mut session) {
                            WaitResult::Timeout | WaitResult::Reconnect => {
                                should_connect = true;
                                auto_reconnect = config.auto_reconnect;
                            }
                            WaitResult::ReconnectWith(new_config) => {
                                config = new_config;
                                session.config = config.clone();
                                should_connect = true;
                                auto_reconnect = config.auto_reconnect;
                                attempts = 0;
                            }
                            WaitResult::Disconnect => {
                                should_connect = false;
                                auto_reconnect = false;
                            }
                            WaitResult::Shutdown => break,
                        }
                    } else {
                        should_connect = false;
                    }
                }
            }
        } else {
            match rx.recv() {
                Ok(ThreadCommand::Input(text)) => {
                    session.local_error(format!(
                        "Hors ligne : impossible d'envoyer « {text} ». Utilisez /reconnect."
                    ));
                }
                Ok(ThreadCommand::Raw(_)) => {
                    session.local_error("Hors ligne : impossible d'envoyer ce message.");
                }
                Ok(ThreadCommand::Privmsg { target, text }) => {
                    session.local_error(format!(
                        "Hors ligne : message pour {target} non envoyé ({text})."
                    ));
                }
                Ok(ThreadCommand::Local { kind, text }) => {
                    session.local_note(kind, text);
                }
                Ok(ThreadCommand::DccSend { .. }) => {
                    session.local_error("Hors ligne : DCC impossible.");
                }
                Ok(ThreadCommand::DccCancel(id)) => {
                    session.cancel_dcc(id);
                }
                Ok(ThreadCommand::Reconnect) => {
                    attempts = 0;
                    auto_reconnect = config.auto_reconnect;
                    should_connect = true;
                }
                Ok(ThreadCommand::ReconnectWith(new_config)) => {
                    config = new_config;
                    session.config = config.clone();
                    attempts = 0;
                    auto_reconnect = config.auto_reconnect;
                    should_connect = true;
                }
                Ok(ThreadCommand::Disconnect) => {}
                Ok(ThreadCommand::Shutdown) | Err(_) => break,
            }
        }
    }
}

enum EndReason {
    Shutdown,
    Reconnect,
    ReconnectWith(ConnectionConfig),
    UserDisconnect(Option<String>),
    Failed { message: String, retryable: bool },
}

enum WaitResult {
    Timeout,
    Reconnect,
    ReconnectWith(ConnectionConfig),
    Disconnect,
    Shutdown,
}

fn wait_for_command(
    rx: &mpsc::Receiver<ThreadCommand>,
    duration: Duration,
    session: &mut Session,
) -> WaitResult {
    let deadline = Instant::now() + duration;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return WaitResult::Timeout;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(ThreadCommand::Input(text)) => {
                session.local_error(format!("Connexion perdue ; « {text} » n'a pas été envoyé."));
            }
            Ok(ThreadCommand::Raw(_)) => {}
            Ok(ThreadCommand::Local { kind, text }) => session.local_note(kind, text),
            Ok(ThreadCommand::Privmsg { .. }) => {
                session.local_error("Connexion perdue : message non envoyé.");
            }
            Ok(ThreadCommand::DccCancel(id)) => session.cancel_dcc(id),
            Ok(ThreadCommand::DccSend { .. }) => {
                session.local_error("Connexion perdue : DCC impossible.");
            }
            Ok(ThreadCommand::Reconnect) => return WaitResult::Reconnect,
            Ok(ThreadCommand::ReconnectWith(config)) => return WaitResult::ReconnectWith(config),
            Ok(ThreadCommand::Disconnect) => return WaitResult::Disconnect,
            Ok(ThreadCommand::Shutdown) => return WaitResult::Shutdown,
            Err(mpsc::RecvTimeoutError::Timeout) => return WaitResult::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => return WaitResult::Shutdown,
        }
    }
}

fn connect_and_run(
    session: &mut Session,
    config: &ConnectionConfig,
    rx: &mpsc::Receiver<ThreadCommand>,
) -> EndReason {
    session.note_connecting();
    let tcp = match connect_tcp(&config.host, config.port, config.proxy.as_deref()) {
        Ok(tcp) => tcp,
        Err(error) => {
            return EndReason::Failed {
                message: format!("connexion impossible : {error}"),
                retryable: true,
            };
        }
    };
    let mut wire = match build_wire(tcp, config) {
        Ok(wire) => wire,
        Err(error) => {
            return EndReason::Failed {
                message: error,
                retryable: false,
            };
        }
    };
    session.on_connected();
    for line in session.initial_lines() {
        if let Err(error) = send_line(&mut wire, &line) {
            return EndReason::Failed {
                message: format!("écriture initiale impossible : {error}"),
                retryable: !config.tls,
            };
        }
    }
    if let Err(error) = wire.set_read_timeout(READ_POLL) {
        log::debug!("réglage du délai de lecture impossible : {error}");
    }

    let mut pending: Vec<u8> = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    loop {
        // Commandes de l'interface.
        loop {
            match rx.try_recv() {
                Ok(ThreadCommand::Input(text)) => {
                    let outcome = session.handle_user_input(&text);
                    for line in outcome.lines {
                        if let Err(error) = send_line(&mut wire, &line) {
                            return EndReason::Failed {
                                message: format!("envoi impossible : {error}"),
                                retryable: true,
                            };
                        }
                    }
                    match outcome.action {
                        ConnectionAction::None => {}
                        ConnectionAction::Reconnect => {
                            let _ = send_line(&mut wire, "QUIT :reconnexion");
                            return EndReason::Reconnect;
                        }
                        ConnectionAction::Disconnect => {
                            return EndReason::UserDisconnect(Some(
                                "déconnexion demandée".to_string(),
                            ));
                        }
                    }
                }
                Ok(ThreadCommand::Raw(line)) => {
                    if let Err(error) = send_line(&mut wire, &line) {
                        return EndReason::Failed {
                            message: format!("envoi impossible : {error}"),
                            retryable: true,
                        };
                    }
                }
                Ok(ThreadCommand::Local { kind, text }) => session.local_note(kind, text),
                Ok(ThreadCommand::Privmsg { target, text }) => {
                    for line in session.send_privmsg(&target, &text) {
                        if let Err(error) = send_line(&mut wire, &line) {
                            return EndReason::Failed {
                                message: format!("envoi impossible : {error}"),
                                retryable: true,
                            };
                        }
                    }
                }
                Ok(ThreadCommand::DccSend { nick, path }) => {
                    match session.start_dcc(&nick, &path) {
                        Ok(lines) => {
                            for line in lines {
                                if let Err(error) = send_line(&mut wire, &line) {
                                    return EndReason::Failed {
                                        message: format!("envoi DCC impossible : {error}"),
                                        retryable: true,
                                    };
                                }
                            }
                        }
                        Err(error) => session.local_error(error),
                    }
                }
                Ok(ThreadCommand::DccCancel(id)) => session.cancel_dcc(id),
                Ok(ThreadCommand::Reconnect) => {
                    let _ = send_line(&mut wire, "QUIT :reconnexion");
                    return EndReason::Reconnect;
                }
                Ok(ThreadCommand::ReconnectWith(config)) => {
                    let _ = send_line(&mut wire, "QUIT :reconnexion");
                    return EndReason::ReconnectWith(config);
                }
                Ok(ThreadCommand::Disconnect) => {
                    let _ = send_line(&mut wire, "QUIT :déconnexion");
                    return EndReason::UserDisconnect(Some("déconnexion demandée".to_string()));
                }
                Ok(ThreadCommand::Shutdown) => {
                    let _ = send_line(&mut wire, "QUIT :fermeture du client");
                    return EndReason::Shutdown;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = send_line(&mut wire, "QUIT :fermeture du client");
                    return EndReason::Shutdown;
                }
            }
        }

        for line in session.tick() {
            if let Err(error) = send_line(&mut wire, &line) {
                return EndReason::Failed {
                    message: format!("envoi impossible : {error}"),
                    retryable: true,
                };
            }
        }

        match wire.read(&mut buffer) {
            Ok(0) => {
                return EndReason::Failed {
                    message: "connexion fermée par le serveur".to_string(),
                    retryable: true,
                };
            }
            Ok(count) => {
                pending.extend_from_slice(&buffer[..count]);
                while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut raw: Vec<u8> = pending.drain(..=position).collect();
                    raw.pop();
                    if raw.last() == Some(&b'\r') {
                        raw.pop();
                    }
                    let line = String::from_utf8_lossy(&raw);
                    if line.is_empty() {
                        continue;
                    }
                    for reply in session.on_line(&line) {
                        if let Err(error) = send_line(&mut wire, &reply) {
                            return EndReason::Failed {
                                message: format!("envoi impossible : {error}"),
                                retryable: true,
                            };
                        }
                    }
                }
                if pending.len() > 64 * 1024 {
                    pending.clear();
                    session.local_error("Ligne IRC trop longue : tampon réinitialisé.");
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => {
                return EndReason::Failed {
                    message: format!("lecture impossible : {error}"),
                    retryable: true,
                };
            }
        }

        if session.timed_out() {
            return EndReason::Failed {
                message: "délai réseau dépassé".to_string(),
                retryable: true,
            };
        }
    }
}

fn connect_tcp(host: &str, port: u16, proxy: Option<&str>) -> Result<TcpStream, String> {
    let Some(proxy) = proxy.map(str::trim).filter(|proxy| !proxy.is_empty()) else {
        return connect_direct(host, port);
    };
    let proxy = proxy
        .strip_prefix("socks5://")
        .or_else(|| proxy.strip_prefix("socks5h://"))
        .unwrap_or(proxy);
    let (credentials, endpoint) = match proxy.rsplit_once('@') {
        Some((credentials, endpoint)) => (Some(credentials), endpoint),
        None => (None, proxy),
    };
    let (proxy_host, proxy_port) =
        split_host_port(endpoint).ok_or_else(|| format!("proxy SOCKS5 invalide : {endpoint}"))?;
    let mut stream = connect_direct(&proxy_host, proxy_port)?;
    socks5_handshake(&mut stream, host, port, credentials)?;
    Ok(stream)
}

fn connect_direct(host: &str, port: u16) -> Result<TcpStream, String> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("résolution DNS de {host} impossible : {error}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("aucune adresse pour {host}"));
    }
    let mut last_error = String::new();
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                return Ok(stream);
            }
            Err(error) => last_error = format!("{addr} : {error}"),
        }
    }
    Err(last_error)
}

fn split_host_port(endpoint: &str) -> Option<(String, u16)> {
    if let Some(rest) = endpoint.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        let port = rest.strip_prefix(':')?.parse().ok()?;
        return Some((host.to_string(), port));
    }
    let (host, port) = endpoint.rsplit_once(':')?;
    Some((host.to_string(), port.parse().ok()?))
}

fn socks5_handshake(
    stream: &mut TcpStream,
    host: &str,
    port: u16,
    credentials: Option<&str>,
) -> Result<(), String> {
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let (user, password) = credentials
        .map(|value| value.split_once(':').unwrap_or((value, "")))
        .map(|(user, password)| (user.as_bytes(), password.as_bytes()))
        .unwrap_or((&[], &[]));
    let use_auth = !user.is_empty();
    let greeting: &[u8] = if use_auth {
        &[0x05, 0x02, 0x00, 0x02]
    } else {
        &[0x05, 0x01, 0x00]
    };
    stream
        .write_all(greeting)
        .map_err(|error| format!("proxy SOCKS5 : écriture impossible : {error}"))?;
    let mut reply = [0u8; 2];
    stream
        .read_exact(&mut reply)
        .map_err(|error| format!("proxy SOCKS5 : réponse illisible : {error}"))?;
    if reply[0] != 0x05 {
        return Err("proxy SOCKS5 : version invalide".into());
    }
    match reply[1] {
        0x00 => {}
        0x02 if use_auth => {
            if user.len() > 255 || password.len() > 255 {
                return Err("proxy SOCKS5 : identifiants trop longs".into());
            }
            let mut auth = Vec::with_capacity(3 + user.len() + password.len());
            auth.extend_from_slice(&[0x01, user.len() as u8]);
            auth.extend_from_slice(user);
            auth.push(password.len() as u8);
            auth.extend_from_slice(password);
            stream
                .write_all(&auth)
                .map_err(|error| format!("proxy SOCKS5 : authentification impossible : {error}"))?;
            let mut auth_reply = [0u8; 2];
            stream.read_exact(&mut auth_reply).map_err(|error| {
                format!("proxy SOCKS5 : réponse d'authentification illisible : {error}")
            })?;
            if auth_reply[1] != 0x00 {
                return Err("proxy SOCKS5 : authentification refusée".into());
            }
        }
        0xff => return Err("proxy SOCKS5 : aucune méthode acceptée".into()),
        _ => return Err("proxy SOCKS5 : méthode d'authentification non supportée".into()),
    }

    if host.len() > 255 {
        return Err("proxy SOCKS5 : nom d'hôte trop long".into());
    }
    let mut request = Vec::with_capacity(7 + host.len());
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host.len() as u8]);
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    stream
        .write_all(&request)
        .map_err(|error| format!("proxy SOCKS5 : connexion impossible : {error}"))?;
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .map_err(|error| format!("proxy SOCKS5 : réponse de connexion illisible : {error}"))?;
    if header[0] != 0x05 || header[1] != 0x00 {
        return Err(format!(
            "proxy SOCKS5 : connexion refusée (code {})",
            header[1]
        ));
    }
    let address_len = match header[3] {
        0x01 => 4,
        0x03 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .map_err(|error| format!("proxy SOCKS5 : adresse illisible : {error}"))?;
            usize::from(length[0])
        }
        0x04 => 16,
        other => return Err(format!("proxy SOCKS5 : type d'adresse inconnu ({other})")),
    };
    let mut discard = vec![0u8; address_len + 2];
    stream
        .read_exact(&mut discard)
        .map_err(|error| format!("proxy SOCKS5 : fin de réponse illisible : {error}"))?;
    Ok(())
}

enum Wire {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Wire {
    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.set_read_timeout(Some(timeout)),
            Self::Tls(stream) => stream.sock.set_read_timeout(Some(timeout)),
        }
    }
}

impl Read for Wire {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn build_wire(tcp: TcpStream, config: &ConnectionConfig) -> Result<Wire, String> {
    if !config.tls {
        return Ok(Wire::Plain(tcp));
    }
    let server_name = ServerName::try_from(config.host.clone())
        .map_err(|_| format!("nom de serveur TLS invalide : {}", config.host))?;
    let tls_config = client_config(config.verify_tls)?;
    let connection = ClientConnection::new(tls_config, server_name)
        .map_err(|error| format!("initialisation TLS impossible : {error}"))?;
    let _ = tcp.set_read_timeout(Some(CONNECT_TIMEOUT));
    Ok(Wire::Tls(Box::new(StreamOwned::new(connection, tcp))))
}

fn client_config(verify: bool) -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("versions TLS indisponibles : {error}"))?;
    if verify {
        let mut roots = RootCertStore::empty();
        roots
            .roots
            .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Ok(Arc::new(
            builder.with_root_certificates(roots).with_no_client_auth(),
        ))
    } else {
        Ok(Arc::new(
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifier(provider)))
                .with_no_client_auth(),
        ))
    }
}

#[derive(Debug)]
struct NoVerifier(Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn send_line(wire: &mut Wire, line: &str) -> io::Result<()> {
    let line = truncate_irc_line(line);
    wire.write_all(line.as_bytes())?;
    wire.write_all(b"\r\n")?;
    wire.flush()
}
