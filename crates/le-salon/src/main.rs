//! Le Salon — point d'entrée de l'application de bureau.

mod app;
mod assets;
mod channel_list;
mod chat;
mod connect_form;
mod emoji;
mod i18n;
mod menu;
mod preferences;
mod schedule;
mod text_input;
mod theme;
mod tooltip;
mod tray;

use std::env;

use gpui::{
    App, AppContext, Application, Bounds, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};

use ircv3_core::{APP_NAME, Config, VERSION, config::parse_channels};

use crate::app::AppRoot;

#[derive(Default)]
struct Cli {
    server: Option<String>,
    port: Option<u16>,
    tls: Option<bool>,
    insecure: bool,
    nick: Option<String>,
    channels: Option<String>,
    sasl_user: Option<String>,
    sasl_password: Option<String>,
    x11: bool,
}

fn parse_cli() -> Result<(Cli, bool), String> {
    let mut cli = Cli::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--server" | "-s" => {
                cli.server = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --server".to_string())?,
                );
            }
            "--port" | "-p" => {
                cli.port = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --port".to_string())?
                        .parse()
                        .map_err(|_| "port invalide".to_string())?,
                );
            }
            "--nick" | "-n" => {
                cli.nick = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --nick".to_string())?,
                );
            }
            "--channel" | "-c" => {
                cli.channels = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --channel".to_string())?,
                );
            }
            "--sasl-user" | "-U" => {
                cli.sasl_user = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --sasl-user".to_string())?,
                );
            }
            "--sasl-password" | "-P" => {
                cli.sasl_password = Some(
                    args.next()
                        .ok_or_else(|| "valeur manquante pour --sasl-password".to_string())?,
                );
            }
            "--tls" => cli.tls = Some(true),
            "--no-tls" => cli.tls = Some(false),
            "--insecure" | "-k" => cli.insecure = true,
            "--x11" => cli.x11 = true,
            "--version" | "-V" => {
                println!("{APP_NAME} {VERSION}");
                return Ok((cli, true));
            }
            "--help" | "-h" => {
                print_help();
                return Ok((cli, true));
            }
            other => return Err(format!("argument inconnu : {other}")),
        }
    }
    Ok((cli, false))
}

fn print_help() {
    println!(
        "{APP_NAME} {VERSION} — client IRCv3 en Rust avec GPUI\n\
         \n\
         Usage : le-salon [options]\n\
         \n\
         Options :\n\
           -s, --server HÔTE        connexion immédiate à ce serveur\n\
           -p, --port PORT          port (défaut 6697 avec TLS, 6667 sinon)\n\
           -n, --nick PSEUDO        pseudo\n\
           -c, --channel #salon     salons à rejoindre (virgules)\n\
           -U, --sasl-user COMPTE   compte SASL PLAIN\n\
           -P, --sasl-password MDP  mot de passe SASL (cette session ; préférer LE_SALON_SASL_PASSWORD)\n\
               --tls / --no-tls     activer/désactiver TLS (cette session)\n\
           -k, --insecure           ne pas vérifier le certificat (cette session ; SASL refusé)\n\
               --x11                forcer le backend X11 (au lieu de Wayland)\n\
           -V, --version            afficher la version\n\
           -h, --help               afficher cette aide\n\
         \n\
         Sécurité : SASL exige TLS et la vérification du certificat.\n\
         Les options --no-tls, --insecure et --sasl-password ne sont pas enregistrées."
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let (cli, exit) = match parse_cli() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    if exit {
        return;
    }
    if cli.x11 {
        // GPUI choisit son backend au démarrage : Wayland est prioritaire dès
        // que WAYLAND_DISPLAY est présent. On le retire pour forcer X11/XWayland.
        unsafe {
            env::remove_var("WAYLAND_DISPLAY");
        }
    }

    // Préférer la variable d'environnement au flag : un mot de passe passé en
    // argument est visible dans `ps` par les autres utilisateurs locaux.
    let cli_sasl_password = cli
        .sasl_password
        .clone()
        .or_else(|| env::var("LE_SALON_SASL_PASSWORD").ok());

    let initial = cli.server.as_ref().map(|server| {
        let config = Config::load();
        // Ne jamais réutiliser aveuglément le secret du dernier réseau : on
        // charge uniquement celui du couple (hôte, compte) demandé.
        let mut connection = config.connection_without_secret();
        connection.host = server.clone();
        connection.nick = cli.nick.clone().unwrap_or(connection.nick);
        connection.user = connection.nick.clone();
        if let Some(tls) = cli.tls {
            connection.tls = tls;
        }
        connection.verify_tls = !cli.insecure;
        if let Some(channels) = &cli.channels {
            connection.channels = parse_channels(channels);
        }
        if let Some(user) = cli.sasl_user.clone() {
            connection.sasl_account = Some(user);
        }
        connection = connection.normalized();
        connection.port = cli.port.unwrap_or(if connection.tls { 6697 } else { 6667 });
        if let Some(password) = cli_sasl_password.clone() {
            // Flag/env ponctuel : jamais écrit dans la configuration.
            connection.sasl_password = Some(password);
        } else if connection.has_sasl() {
            let account = connection.sasl_account.clone().unwrap_or_default();
            connection.sasl_password = ircv3_core::secrets::get(&connection.host, &account)
                .ok()
                .flatten();
        }
        connection
    });

    if let Some(connection) = &initial
        && let Err(error) = connection.security_check()
    {
        eprintln!("{error}");
        std::process::exit(2);
    }

    Application::new()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            app::install_bindings(cx);
            cx.set_menus(vec![Menu {
                name: "IRCv3".into(),
                items: vec![MenuItem::action("Quitter", app::AppQuit)],
            }]);

            let bounds = Bounds::centered(None, size(px(1180.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("{APP_NAME} {VERSION}").into()),
                    ..Default::default()
                }),
                app_id: Some("le-salon".into()),
                window_min_size: Some(size(px(820.), px(520.))),
                ..Default::default()
            };
            let initial = initial.clone();
            let window = cx
                .open_window(options, move |window, cx| {
                    cx.new(|cx| AppRoot::new(window, cx, initial.clone()))
                })
                .expect("impossible d'ouvrir la fenêtre GPUI");
            let view = window
                .update(cx, |_, _, cx| cx.entity())
                .expect("vue racine introuvable");
            window
                .update(cx, |_, window, cx| {
                    window.on_window_should_close(cx, move |window, cx| {
                        view.update(cx, |root, cx| root.hide_to_tray(window, cx))
                    });
                })
                .ok();
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}
