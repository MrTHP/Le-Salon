//! Icône de barre système (StatusNotifierItem) sous Linux.
//!
//! macOS n'expose pas la même API publique dans GPUI ; les notifications
//! natives y fonctionnent en revanche via `osascript`.

use std::sync::mpsc;

#[cfg(target_os = "linux")]
use crate::i18n;

#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    Show,
    Minimize,
    ToggleNotifications,
    DisconnectAll,
    Quit,
}

pub type TrayReceiver = mpsc::Receiver<TrayCommand>;

pub struct TrayController {
    #[cfg(target_os = "linux")]
    inner: ksni::blocking::Handle<AppTray>,
}

impl TrayController {
    pub fn set_language(&self, lang: ircv3_core::Lang) {
        #[cfg(target_os = "linux")]
        let _ = self.inner.update(|tray| {
            tray.lang = lang;
        });
        #[cfg(not(target_os = "linux"))]
        let _ = lang;
    }
}

pub fn start() -> (Option<TrayController>, TrayReceiver) {
    let (tx, rx) = mpsc::channel();
    #[cfg(target_os = "linux")]
    {
        use ksni::blocking::TrayMethods as _;
        let tray = AppTray {
            tx,
            lang: i18n::lang(),
            title: "Le Salon".into(),
        };
        match tray.spawn() {
            Ok(inner) => (Some(TrayController { inner }), rx),
            Err(error) => {
                log::debug!("barre système indisponible : {error}");
                (None, rx)
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = tx;
        (None, rx)
    }
}

#[cfg(target_os = "linux")]
struct AppTray {
    tx: mpsc::Sender<TrayCommand>,
    lang: ircv3_core::Lang,
    title: String,
}

#[cfg(target_os = "linux")]
impl ksni::Tray for AppTray {
    fn id(&self) -> String {
        "le-salon".into()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icon()]
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.send(TrayCommand::Show);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        let fr = self.lang == ircv3_core::Lang::Fr;
        vec![
            StandardItem {
                label: if fr {
                    "Afficher Le Salon"
                } else {
                    "Show Le Salon"
                }
                .into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCommand::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if fr { "Réduire" } else { "Minimize" }.into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCommand::Minimize);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: if fr { "Notifications" } else { "Notifications" }.into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCommand::ToggleNotifications);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if fr {
                    "Déconnecter tous les réseaux"
                } else {
                    "Disconnect all networks"
                }
                .into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCommand::DisconnectAll);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if fr { "Quitter" } else { "Quit" }.into(),
                activate: Box::new(|this: &mut Self| {
                    let _ = this.tx.send(TrayCommand::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Petite icône « canapé » dessinée en pixels pour la barre système.
/// Les couleurs reprennent celles de l'icône de l'application.
#[cfg(target_os = "linux")]
fn icon() -> ksni::Icon {
    const WIDTH: i32 = 32;
    const HEIGHT: i32 = 32;

    const DARK: [u8; 4] = [0xff, 0x10, 0x16, 0x1e];
    const CREAM: [u8; 4] = [0xff, 0xf2, 0xe6, 0xd0];
    const BEIGE: [u8; 4] = [0xff, 0xe0, 0xcc, 0xac];
    const SEAM: [u8; 4] = [0xff, 0xc7, 0xac, 0x83];
    const WOOD: [u8; 4] = [0xff, 0x8b, 0x6b, 0x4a];
    const BLUE: [u8; 4] = [0xff, 0x58, 0xb9, 0xff];
    const GREEN: [u8; 4] = [0xff, 0x7e, 0xe7, 0x87];
    const SHADOW: [u8; 4] = [0x60, 0x00, 0x00, 0x00];

    let mut data = vec![0u8; (WIDTH * HEIGHT * 4) as usize];

    // Ombre au sol, puis contour sombre du canapé.
    fill_rect(&mut data, WIDTH, 4, 26, 27, 29, SHADOW);
    fill_rect(&mut data, WIDTH, 6, 4, 26, 15, DARK);
    fill_rect(&mut data, WIDTH, 3, 9, 9, 25, DARK);
    fill_rect(&mut data, WIDTH, 23, 9, 29, 25, DARK);
    fill_rect(&mut data, WIDTH, 5, 14, 27, 25, DARK);
    fill_rect(&mut data, WIDTH, 7, 23, 11, 28, DARK);
    fill_rect(&mut data, WIDTH, 21, 23, 25, 28, DARK);

    // Dossier et coussins décoratifs.
    fill_rect(&mut data, WIDTH, 7, 5, 25, 14, CREAM);
    fill_rect(&mut data, WIDTH, 9, 7, 14, 13, BLUE);
    fill_rect(&mut data, WIDTH, 18, 7, 23, 13, GREEN);

    // Accoudoirs.
    fill_rect(&mut data, WIDTH, 4, 10, 8, 24, BEIGE);
    fill_rect(&mut data, WIDTH, 24, 10, 28, 24, BEIGE);

    // Assise et séparation des coussins.
    fill_rect(&mut data, WIDTH, 6, 15, 26, 24, CREAM);
    fill_rect(&mut data, WIDTH, 15, 15, 16, 23, SEAM);
    fill_rect(&mut data, WIDTH, 6, 22, 26, 23, BEIGE);

    // Pieds.
    fill_rect(&mut data, WIDTH, 8, 24, 10, 27, WOOD);
    fill_rect(&mut data, WIDTH, 22, 24, 24, 27, WOOD);

    ksni::Icon {
        width: WIDTH,
        height: HEIGHT,
        data,
    }
}

#[cfg(target_os = "linux")]
fn fill_rect(data: &mut [u8], width: i32, x0: i32, y0: i32, x1: i32, y1: i32, color: [u8; 4]) {
    let max_y = (data.len() / (width as usize * 4)) as i32;
    for y in y0.max(0)..=y1.min(max_y - 1) {
        for x in x0.max(0)..=x1.min(width - 1) {
            let offset = (y as usize * width as usize + x as usize) * 4;
            data[offset..offset + 4].copy_from_slice(&color);
        }
    }
}
