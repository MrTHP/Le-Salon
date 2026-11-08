//! Petit client console sans interface, utilisé par les tests de bout en bout.
//!
//! Exemple :
//!   cargo run -p ircv3-core --example probe -- \
//!       --host 127.0.0.1 --port 6668 --no-tls --nick test --channel '#test' --seconds 6

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ircv3_core::{
    client,
    config::{Config, ConnectionConfig},
    model::{AppModel, BufferId, ConnectionStatus, LineKind},
};

fn main() {
    let mut host = "127.0.0.1".to_string();
    let mut port = 6668u16;
    let mut tls = false;
    let mut verify = true;
    let mut nick = "probe".to_string();
    let mut channel = "#test".to_string();
    let mut sasl_user: Option<String> = None;
    let mut sasl_pass: Option<String> = None;
    let mut seconds = 6u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => host = args.next().unwrap_or(host),
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--tls" => tls = true,
            "--no-tls" => tls = false,
            "--insecure" => verify = false,
            "--nick" => nick = args.next().unwrap_or(nick),
            "--channel" => channel = args.next().unwrap_or(channel),
            "--sasl-user" => sasl_user = args.next(),
            "--sasl-pass" => sasl_pass = args.next(),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            "--help" | "-h" => {
                println!(
                    "usage: probe [--host H] [--port P] [--tls|--no-tls] [--insecure] \
                     [--nick N] [--channel #c] [--sasl-user U --sasl-pass P] [--seconds N]"
                );
                return;
            }
            other => eprintln!("argument ignoré : {other}"),
        }
    }

    let mut config = Config::default();
    config.server = host.clone();
    config.port = port;
    config.tls = tls;
    config.verify_tls = verify;
    config.nick = nick.clone();
    config.user = nick.clone();
    config.realname = "IRCv3 probe".into();
    config.channels = channel.clone();
    config.sasl_account = sasl_user.clone().unwrap_or_default();
    let connection = ConnectionConfig {
        host,
        port,
        tls,
        verify_tls: verify,
        nick,
        user: config.user.clone(),
        realname: config.realname.clone(),
        sasl_account: sasl_user,
        sasl_password: sasl_pass,
        channels: vec![channel],
        auto_reconnect: false,
    };

    let model = Arc::new(Mutex::new(AppModel::new(config)));
    let (wake_tx, wake_rx) = async_channel::bounded(1);
    let network_id = model.lock().unwrap().add_network(connection.clone());
    let handle = client::spawn(network_id, connection, model.clone(), wake_tx);

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut printed: HashMap<BufferId, usize> = HashMap::new();
    let mut last_status = None;

    while Instant::now() < deadline {
        let _ = wake_rx.try_recv();
        {
            let model = model.lock().unwrap();
            if let Some(network) = model.network(network_id) {
                let status = (network.status, network.status_text.clone());
                if last_status.as_ref() != Some(&status) {
                    println!("[status] {} — {}", status.0.label(), status.1);
                    last_status = Some(status);
                }
            }
            let mut buffers: Vec<_> = model.buffers.values().collect();
            buffers.sort_by_key(|buffer| buffer.id);
            for buffer in buffers {
                let start = printed.get(&buffer.id).copied().unwrap_or(0);
                for line in buffer.messages.iter().skip(start) {
                    let who = line.nick.as_deref().unwrap_or("—");
                    let tag = match line.kind {
                        LineKind::Message | LineKind::History => "",
                        LineKind::Action => " *",
                        LineKind::Notice => " -notice-",
                        LineKind::Error => " !",
                        _ => " ·",
                    };
                    println!("[{}]{} <{}> {}", buffer.name, tag, who, line.text);
                }
                printed.insert(buffer.id, buffer.messages.len());
            }
            if model
                .network(network_id)
                .is_some_and(|n| n.status == ConnectionStatus::Disconnected)
                && last_status.is_some()
            {
                // On laisse une chance aux dernières lignes d'arriver.
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    handle.disconnect();
    std::thread::sleep(Duration::from_millis(300));
    println!("[fin] probe terminé");
}
