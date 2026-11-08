//! DCC SEND (émission directe d'un fichier).
//!
//! Ouvre un port TCP, envoie l'offre CTCP `DCC SEND` au destinataire, attend
//! sa connexion puis streame le fichier avec une progression dans le modèle.
//! Taille 64 bits, annulation coopérative. Le port doit être joignable par le
//! destinataire (LAN, IP publique + redirection, pas de CGNAT).

use std::{
    fs::{self, File},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Local;

use crate::{
    Wake,
    model::{AppModel, DccStatus, DccTransfer, NetworkId},
};

pub const DCC_TIMEOUT: Duration = Duration::from_secs(120);
const CHUNK: usize = 256 * 1024;

/// Prépare l'offre et démarre le thread d'envoi.
///
/// Retourne la charge CTCP (sans les `\x01`) à envoyer au destinataire.
pub fn start_send(
    network_id: NetworkId,
    model: Arc<Mutex<AppModel>>,
    wake: Wake,
    nick: &str,
    path: &Path,
) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("fichier inaccessible : {error}"))?;
    if !metadata.is_file() {
        return Err("ce n'est pas un fichier".into());
    }
    let size = metadata.len();
    let filename = sanitize_filename(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("fichier"),
    );
    let ip = local_ipv4();
    // On n'écoute que sur l'interface locale annoncée dans l'offre : le port
    // n'est pas exposé sur toutes les interfaces de la machine. Le premier
    // pair qui se connecte reçoit le fichier, comme le veut DCC ; c'est
    // pourquoi l'offre doit rester privée et le fichier non sensible.
    let listener = TcpListener::bind((ip, 0))
        .map_err(|error| format!("impossible d'ouvrir le port DCC sur {ip} : {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("adresse DCC illisible : {error}"))?
        .port();
    let cancel = Arc::new(AtomicBool::new(false));
    let transfer = DccTransfer {
        id: 0,
        network: network_id,
        nick: nick.to_string(),
        filename: filename.clone(),
        path: path.to_path_buf(),
        size,
        sent: 0,
        status: DccStatus::Waiting,
        error: None,
        started_at: Local::now(),
        cancel: cancel.clone(),
    };
    let id = {
        let mut model = model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        model.add_dcc(transfer)
    };
    let _ = wake.try_send(());

    let thread_model = model.clone();
    let thread_wake = wake.clone();
    let thread_path = path.to_path_buf();
    std::thread::Builder::new()
        .name(format!("dcc-send-{id}"))
        .spawn(move || {
            run_send(
                id,
                listener,
                thread_path,
                size,
                cancel,
                thread_model,
                thread_wake,
            )
        })
        .map_err(|error| format!("impossible de démarrer DCC : {error}"))?;

    Ok(format!(
        "DCC SEND {} {} {} {}",
        filename,
        u32::from(ip),
        port,
        size
    ))
}

fn run_send(
    id: u64,
    listener: TcpListener,
    path: PathBuf,
    size: u64,
    cancel: Arc<AtomicBool>,
    model: Arc<Mutex<AppModel>>,
    wake: Wake,
) {
    let _ = listener.set_nonblocking(true);
    let deadline = Instant::now() + DCC_TIMEOUT;
    let stream = loop {
        if cancel.load(Ordering::Relaxed) {
            set_status(
                &model,
                &wake,
                id,
                DccStatus::Canceled,
                Some("annulé".into()),
                0,
            );
            return;
        }
        if Instant::now() >= deadline {
            set_status(
                &model,
                &wake,
                id,
                DccStatus::Failed,
                Some("aucune connexion DCC reçue".into()),
                0,
            );
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                set_status(
                    &model,
                    &wake,
                    id,
                    DccStatus::Failed,
                    Some(format!("acceptation DCC impossible : {error}")),
                    0,
                );
                return;
            }
        }
    };
    drop(listener);

    match send_stream(id, stream, &path, size, &cancel, &model, &wake) {
        Ok(sent) => {
            set_status(&model, &wake, id, DccStatus::Done, None, sent);
        }
        Err(error) if cancel.load(Ordering::Relaxed) => {
            set_status(&model, &wake, id, DccStatus::Canceled, Some(error), 0);
        }
        Err(error) => {
            set_status(&model, &wake, id, DccStatus::Failed, Some(error), 0);
        }
    }
}

fn send_stream(
    id: u64,
    mut stream: TcpStream,
    path: &Path,
    size: u64,
    cancel: &AtomicBool,
    model: &Arc<Mutex<AppModel>>,
    wake: &Wake,
) -> Result<u64, String> {
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_nodelay(true);
    update(model, wake, id, |transfer| {
        transfer.status = DccStatus::Connected;
        transfer.sent = 0;
    });
    let mut file = File::open(path).map_err(|error| format!("ouverture impossible : {error}"))?;
    let mut buffer = vec![0u8; CHUNK];
    let mut sent = 0u64;
    let mut last_update = Instant::now();
    update(model, wake, id, |transfer| {
        transfer.status = DccStatus::Sending;
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = stream.shutdown(Shutdown::Both);
            return Err("transfert annulé".into());
        }
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("lecture impossible : {error}"))?;
        if count == 0 {
            break;
        }
        stream
            .write_all(&buffer[..count])
            .map_err(|error| format!("connexion interrompue : {error}"))?;
        sent += count as u64;
        if last_update.elapsed() >= Duration::from_millis(200) || sent == size {
            last_update = Instant::now();
            update(model, wake, id, |transfer| {
                transfer.sent = sent;
            });
        }
    }
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    update(model, wake, id, |transfer| {
        transfer.sent = sent;
    });
    Ok(sent)
}

fn update(model: &Arc<Mutex<AppModel>>, wake: &Wake, id: u64, f: impl FnOnce(&mut DccTransfer)) {
    {
        let mut model = model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(transfer) = model.dcc_transfer_mut(id) {
            f(transfer);
        }
        model.touch();
    }
    let _ = wake.try_send(());
}

fn set_status(
    model: &Arc<Mutex<AppModel>>,
    wake: &Wake,
    id: u64,
    status: DccStatus,
    error: Option<String>,
    sent: u64,
) {
    update(model, wake, id, |transfer| {
        transfer.status = status;
        transfer.error = error;
        if status == DccStatus::Done {
            transfer.sent = transfer.size;
        } else if sent > 0 {
            transfer.sent = sent;
        }
    });
}

pub fn sanitize_filename(name: &str) -> String {
    if name.trim().is_empty() {
        return "fichier".into();
    }
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else if c.is_whitespace() {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('.').trim();
    if trimmed.is_empty() {
        "fichier".into()
    } else {
        trimmed.chars().take(200).collect()
    }
}

pub fn local_ipv4() -> Ipv4Addr {
    // UDP connect ne envoie aucun paquet ; il sert seulement à connaître
    // l'adresse de sortie choisie par la table de routage.
    UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("8.8.8.8", 80))?;
            Ok(socket.local_addr()?.ip())
        })
        .ok()
        .and_then(|ip| match ip {
            IpAddr::V4(ip) if !ip.is_loopback() => Some(ip),
            _ => None,
        })
        .unwrap_or(Ipv4Addr::LOCALHOST)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_are_sanitized() {
        assert_eq!(
            sanitize_filename("mon fichier (1).txt"),
            "mon_fichier_(1).txt"
        );
        assert_eq!(sanitize_filename("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_filename("   "), "fichier");
    }

    #[test]
    fn ipv4_is_present() {
        assert!(!local_ipv4().is_unspecified());
    }

    #[test]
    fn dcc_send_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "ircv3-dcc-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("payload.bin");
        let data: Vec<u8> = (0..200_000u32).map(|value| (value % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();

        let model = Arc::new(Mutex::new(crate::model::AppModel::new(
            crate::config::Config::default(),
        )));
        let network = {
            let mut model = model.lock().unwrap();
            model.add_network(crate::config::ConnectionConfig {
                host: "irc.test".into(),
                port: 6667,
                tls: false,
                verify_tls: false,
                nick: "moi".into(),
                user: "moi".into(),
                realname: "Moi".into(),
                sasl_account: None,
                sasl_password: None,
                channels: vec![],
                auto_reconnect: false,
            })
        };
        let (wake, _rx) = async_channel::bounded(1);
        let offer = start_send(network, model.clone(), wake, "alice", &path).unwrap();
        let mut parts = offer.split_whitespace();
        assert_eq!(parts.next(), Some("DCC"));
        assert_eq!(parts.next(), Some("SEND"));
        let filename = parts.next().unwrap();
        assert_eq!(filename, "payload.bin");
        let ip: u32 = parts.next().unwrap().parse().unwrap();
        let port: u16 = parts.next().unwrap().parse().unwrap();
        let size: u64 = parts.next().unwrap().parse().unwrap();
        assert_eq!(size, data.len() as u64);
        assert!(ip > 0);

        let mut stream = TcpStream::connect((Ipv4Addr::from(ip), port)).unwrap();
        let mut received = Vec::new();
        stream.read_to_end(&mut received).unwrap();
        assert_eq!(received, data);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = {
                let model = model.lock().unwrap();
                model.dcc.first().map(|transfer| transfer.status)
            };
            if status == Some(DccStatus::Done) {
                break;
            }
            if Instant::now() > deadline {
                panic!("transfert DCC non terminé : {status:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
