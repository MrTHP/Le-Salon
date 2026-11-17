//! Fenêtre de repli quand aucun pilote Vulkan n'est disponible.
//!
//! GPUI/Blade ne peut pas dessiner sans Vulkan. Plutôt que de laisser
//! l'application quitter ou afficher une fenêtre noire, on ouvre une fenêtre
//! X11 légère qui explique comment installer le pilote graphique.

#[cfg(target_os = "linux")]
pub fn show() -> anyhow::Result<()> {
    use anyhow::Context as _;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        AtomEnum, ChangeGCAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask,
        PropMode, Rectangle, WindowClass,
    };
    use x11rb::wrapper::ConnectionExt as _;

    const WIDTH: u16 = 760;
    const HEIGHT: u16 = 430;
    const BG: u32 = 0x00f6_f1e7;
    const PANEL: u32 = 0x00ec_e5d8;
    const TEXT: u32 = 0x002f_2a24;
    const MUTED: u32 = 0x006c_6458;
    const ACCENT: u32 = 0x009a_5b2e;
    const ACCENT_TEXT: u32 = 0x00ff_faf2;

    let (conn, screen_num) = x11rb::connect(None).context("connexion au serveur X11 impossible")?;
    let screen = &conn.setup().roots[screen_num];

    let win = conn.generate_id()?;
    let x = ((screen.width_in_pixels as i32 - WIDTH as i32) / 2).max(0) as i16;
    let y = ((screen.height_in_pixels as i32 - HEIGHT as i32) / 2).max(0) as i16;
    let aux = CreateWindowAux::new()
        .background_pixel(BG)
        .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY | EventMask::KEY_PRESS);
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        screen.root,
        x,
        y,
        WIDTH,
        HEIGHT,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &aux,
    )?;

    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        b"Le Salon - Vulkan required",
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        b"le-salon\0LeSalon\0",
    )?;

    let protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
    let delete_window = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
    conn.change_property32(
        PropMode::REPLACE,
        win,
        protocols,
        AtomEnum::ATOM,
        &[delete_window],
    )?;

    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new().foreground(TEXT).background(BG))?;

    conn.map_window(win)?;
    conn.flush()?;

    let lines = [
        "Le Salon needs a Vulkan-capable graphics driver.",
        "",
        "Install the Vulkan driver for your GPU, then restart Le Salon:",
        "",
        "  Debian / Ubuntu : sudo apt install mesa-vulkan-drivers",
        "  Fedora          : sudo dnf install mesa-vulkan-drivers",
        "  Arch            : sudo pacman -S vulkan-icd-loader",
        "",
        "Le Salon necessite un pilote Vulkan. Installez-le puis relancez.",
        "",
        "The IRC interface cannot start without a working Vulkan device.",
    ];

    loop {
        let event = conn
            .wait_for_event()
            .context("attente d'un événement X11")?;
        match event {
            Event::Expose(event) if event.count == 0 => {
                conn.clear_area(true, win, 0, 0, 0, 0)?;

                conn.change_gc(
                    gc,
                    &ChangeGCAux::new().foreground(ACCENT).background(ACCENT),
                )?;
                conn.poly_fill_rectangle(
                    win,
                    gc,
                    &[Rectangle {
                        x: 0,
                        y: 0,
                        width: WIDTH,
                        height: 72,
                    }],
                )?;
                conn.change_gc(
                    gc,
                    &ChangeGCAux::new()
                        .foreground(ACCENT_TEXT)
                        .background(ACCENT),
                )?;
                conn.image_text8(win, gc, 28, 44, b"Le Salon")?;

                conn.change_gc(gc, &ChangeGCAux::new().foreground(TEXT).background(BG))?;
                for (index, line) in lines.iter().enumerate() {
                    conn.image_text8(win, gc, 32, 120 + 22 * index as i16, line.as_bytes())?;
                }

                conn.change_gc(gc, &ChangeGCAux::new().foreground(MUTED).background(PANEL))?;
                conn.poly_fill_rectangle(
                    win,
                    gc,
                    &[Rectangle {
                        x: 24,
                        y: 372,
                        width: WIDTH - 48,
                        height: 34,
                    }],
                )?;
                conn.change_gc(gc, &ChangeGCAux::new().foreground(MUTED).background(PANEL))?;
                conn.image_text8(
                    win,
                    gc,
                    32,
                    394,
                    b"https://github.com/MrTHP/Le-Salon  -  mesa-vulkan-drivers",
                )?;
                conn.flush()?;
            }
            Event::ClientMessage(event) => {
                if event.data.as_data32()[0] == delete_window {
                    break;
                }
            }
            Event::KeyPress(_) => break,
            _ => {}
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn show() -> anyhow::Result<()> {
    anyhow::bail!("la fenêtre de repli X11 n'est disponible que sous Linux")
}
