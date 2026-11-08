//! Test d'intégration du transport : vrai socket TCP, vraie négociation
//! CAP/SASL, vrai aller-retour de messages.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use ircv3_core::{
    client,
    config::{Config, ConnectionConfig},
    model::{AppModel, BufferKind, ConnectionStatus, DccStatus},
};

#[test]
fn plain_tcp_cap_join_and_message() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let server_received = received.clone();

    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        serve(stream, server_received);
    });

    let config = Config::default();
    let connection = ConnectionConfig {
        host: "127.0.0.1".into(),
        port,
        tls: false,
        verify_tls: false,
        nick: "tester".into(),
        user: "tester".into(),
        realname: "Testeur".into(),
        sasl_account: None,
        sasl_password: None,
        channels: vec!["#test".into()],
        auto_reconnect: false,
    };
    let model = Arc::new(Mutex::new(AppModel::new(config)));
    let (wake_tx, _wake_rx) = async_channel::bounded(1);
    let network_id = model.lock().unwrap().add_network(connection.clone());
    let handle = client::spawn(network_id, connection, model.clone(), wake_tx);

    wait_until(Duration::from_secs(8), || {
        let model = model.lock().unwrap();
        model
            .network(network_id)
            .is_some_and(|network| network.status == ConnectionStatus::Connected)
            && model
                .find_buffer(network_id, BufferKind::Channel, "#test")
                .is_some_and(|id| {
                    model.buffer(id).is_some_and(|buffer| {
                        buffer
                            .messages
                            .iter()
                            .any(|line| line.text.contains("salut"))
                    })
                })
    });

    {
        let model = model.lock().unwrap();
        let channel = model
            .find_buffer(network_id, BufferKind::Channel, "#test")
            .expect("salon #test absent");
        let buffer = model.buffer(channel).unwrap();
        assert!(buffer.user("alice").is_some());
        assert!(buffer.user("tester").is_some());
        assert!(buffer.topic.contains("Salon de test"));
    }

    handle.send_input("bonjour depuis le test");
    wait_until(Duration::from_secs(5), || {
        let model = model.lock().unwrap();
        model
            .find_buffer(network_id, BufferKind::Channel, "#test")
            .and_then(|id| model.buffer(id))
            .is_some_and(|buffer| {
                buffer
                    .messages
                    .iter()
                    .any(|line| line.self_nick && line.text == "bonjour depuis le test")
            })
    });

    let lines = received.lock().unwrap().clone();
    assert!(lines.iter().any(|line| line == "CAP LS 302"));
    assert!(lines.iter().any(|line| line == "JOIN #test"));
    assert!(
        lines
            .iter()
            .any(|line| line == "PRIVMSG #test :bonjour depuis le test")
    );

    // DCC SEND : l'offre part par IRC, puis le fichier arrive par TCP.
    let dir = std::env::temp_dir().join(format!(
        "ircv3-network-dcc-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dcc-test.bin");
    let payload: Vec<u8> = (0..80_000u32).map(|value| (value % 239) as u8).collect();
    std::fs::write(&path, &payload).unwrap();
    handle.dcc_send("alice", path.clone());
    wait_until(Duration::from_secs(5), || {
        received
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("DCC SEND dcc-test.bin"))
    });
    let offer = received
        .lock()
        .unwrap()
        .iter()
        .find(|line| line.contains("DCC SEND dcc-test.bin"))
        .cloned()
        .unwrap();
    let ctcp = offer.split(" :").nth(1).unwrap().trim_matches('\u{01}');
    let parts: Vec<&str> = ctcp.split_whitespace().collect();
    let ip: u32 = parts[3].parse().unwrap();
    let port: u16 = parts[4].parse().unwrap();
    let size: u64 = parts[5].parse().unwrap();
    assert_eq!(size, payload.len() as u64);
    let mut stream = TcpStream::connect((Ipv4Addr::from(ip), port)).unwrap();
    let mut received_file = Vec::new();
    stream.read_to_end(&mut received_file).unwrap();
    assert_eq!(received_file, payload);
    wait_until(Duration::from_secs(5), || {
        let model = model.lock().unwrap();
        model
            .dcc
            .iter()
            .any(|transfer| transfer.status == DccStatus::Done)
    });
    let _ = std::fs::remove_dir_all(dir);

    handle.disconnect();
}

#[test]
fn sasl_without_tls_is_refused_before_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();

    let connection = ConnectionConfig {
        host: "127.0.0.1".into(),
        port,
        tls: false,
        verify_tls: true,
        nick: "tester".into(),
        user: "tester".into(),
        realname: "Testeur".into(),
        sasl_account: Some("tester".into()),
        sasl_password: Some("secret".into()),
        channels: Vec::new(),
        auto_reconnect: false,
    };
    let model = Arc::new(Mutex::new(AppModel::new(Config::default())));
    let (wake_tx, _wake_rx) = async_channel::bounded(1);
    let network_id = model.lock().unwrap().add_network(connection.clone());
    let handle = client::spawn(network_id, connection, model.clone(), wake_tx);

    wait_until(Duration::from_secs(2), || {
        model
            .lock()
            .unwrap()
            .network(network_id)
            .is_some_and(|network| network.status == ConnectionStatus::Disconnected)
    });

    let model = model.lock().unwrap();
    let network = model.network(network_id).unwrap();
    assert_eq!(network.status, ConnectionStatus::Disconnected);
    assert!(network.status_text.contains("SASL"));
    assert!(network.status_text.contains("TLS"));
    drop(model);

    // Aucune connexion TCP ne doit avoir été tentée.
    assert!(listener.accept().is_err());
    handle.disconnect();
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("condition non satisfaite après {timeout:?}");
}

fn serve(stream: TcpStream, received: Arc<Mutex<Vec<String>>>) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut nick = "tester".to_string();
    let mut cap_ended = false;
    let mut sent_welcome = false;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        received.lock().unwrap().push(line.clone());
        let (command, rest) = match line.split_once(' ') {
            Some((command, rest)) => (command.to_ascii_uppercase(), rest),
            None => (line.to_ascii_uppercase(), ""),
        };
        match command.as_str() {
            "CAP" => {
                if rest.starts_with("LS") {
                    send(
                        &mut writer,
                        ":irc.test CAP * LS :message-tags server-time echo-message multi-prefix sasl=PLAIN cap-notify account-notify away-notify extended-join",
                    );
                } else if rest.starts_with("REQ") {
                    let caps = rest
                        .splitn(3, ' ')
                        .nth(2)
                        .unwrap_or("")
                        .trim_start_matches(':');
                    send(&mut writer, &format!(":irc.test CAP * ACK :{caps}"));
                } else if rest.starts_with("END") {
                    cap_ended = true;
                    if !sent_welcome {
                        sent_welcome = true;
                        welcome(&mut writer, &nick);
                    }
                }
            }
            "AUTHENTICATE" => {
                if rest == "PLAIN" {
                    send(&mut writer, "AUTHENTICATE +");
                } else {
                    send(
                        &mut writer,
                        &format!(":irc.test 900 {nick} {nick}!u@h :logged in"),
                    );
                    send(&mut writer, &format!(":irc.test 903 {nick} :SASL success"));
                }
            }
            "NICK" => {
                nick = rest.to_string();
            }
            "USER" => {
                if cap_ended && !sent_welcome {
                    sent_welcome = true;
                    welcome(&mut writer, &nick);
                }
            }
            "JOIN" => {
                let channel = rest.split(',').next().unwrap_or("#test");
                send(
                    &mut writer,
                    &format!(":{nick}!u@h JOIN {channel} tester :Testeur"),
                );
                send(
                    &mut writer,
                    &format!(":irc.test 332 {nick} {channel} :Salon de test"),
                );
                send(
                    &mut writer,
                    &format!(":irc.test 353 {nick} = {channel} :@{nick} alice"),
                );
                send(
                    &mut writer,
                    &format!(":irc.test 366 {nick} {channel} :End of NAMES"),
                );
                send(
                    &mut writer,
                    &format!(":alice!a@h PRIVMSG {channel} :salut {nick}"),
                );
            }
            "PRIVMSG" => {
                let (target, text) = rest.split_once(" :").unwrap_or((rest, ""));
                send(
                    &mut writer,
                    &format!("@time=2026-01-01T00:00:00.000Z :{nick}!u@h PRIVMSG {target} :{text}"),
                );
            }
            "PING" => {
                send(&mut writer, &format!("PONG :{rest}"));
            }
            "QUIT" => break,
            _ => {}
        }
    }
}

fn welcome(writer: &mut TcpStream, nick: &str) {
    send(
        writer,
        &format!(":irc.test 001 {nick} :Bienvenue sur TestNet, {nick}"),
    );
    send(
        writer,
        &format!(
            ":irc.test 005 {nick} CHANTYPES=# PREFIX=(ov)@+ NETWORK=TestNet CASEMAPPING=rfc1459 :supported"
        ),
    );
    send(writer, &format!(":irc.test 375 {nick} :- MOTD -"));
    send(writer, &format!(":irc.test 376 {nick} :End of MOTD"));
}

fn send(writer: &mut TcpStream, line: &str) {
    let _ = writer.write_all(format!("{line}\r\n").as_bytes());
    let _ = writer.flush();
}
