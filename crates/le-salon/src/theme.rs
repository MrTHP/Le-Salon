//! Palettes de thèmes et aides de style.
//!
//! Le thème courant est global (AtomicUsize) : il est changé depuis le
//! sélecteur de thèmes et toutes les vues le relisent au rendu suivant.

use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use gpui::{Hsla, rgb, rgba};
use serde::{Deserialize, Serialize};

use ircv3_core::model::{ConnectionStatus, LineKind};

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub key: &'static str,
    pub name_fr: &'static str,
    pub name_en: &'static str,
    pub bg: u32,
    pub panel: u32,
    pub panel_alt: u32,
    pub input_bg: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_dark: u32,
    pub accent_hover: u32,
    pub on_accent: u32,
    pub green: u32,
    pub yellow: u32,
    pub red: u32,
    pub orange: u32,
    pub purple: u32,
    pub pink: u32,
    pub nicks: [u32; 8],
}

const fn make(
    key: &'static str,
    name_fr: &'static str,
    name_en: &'static str,
    bg: u32,
    panel: u32,
    panel_alt: u32,
    input_bg: u32,
    border: u32,
    text: u32,
    muted: u32,
    faint: u32,
    accent: u32,
    accent_dark: u32,
    accent_hover: u32,
    on_accent: u32,
    green: u32,
    yellow: u32,
    red: u32,
    orange: u32,
    purple: u32,
    pink: u32,
    nicks: [u32; 8],
) -> Theme {
    Theme {
        key,
        name_fr,
        name_en,
        bg,
        panel,
        panel_alt,
        input_bg,
        border,
        text,
        muted,
        faint,
        accent,
        accent_dark,
        accent_hover,
        on_accent,
        green,
        yellow,
        red,
        orange,
        purple,
        pink,
        nicks,
    }
}

pub static THEMES: &[Theme] = &[
    make(
        "nuit",
        "Nuit",
        "Night",
        0x0d1117,
        0x151b23,
        0x1b232d,
        0x10161e,
        0x2a3440,
        0xd7dee8,
        0x8b97a7,
        0x5d6979,
        0x58b9ff,
        0x1d3b52,
        0xd7dee8,
        0x0d1117,
        0x7ee787,
        0xf2cc60,
        0xff7b72,
        0xffa657,
        0xd2a8ff,
        0xff9bce,
        [
            0x79c0ff, 0x7ee787, 0xffa657, 0xd2a8ff, 0xff9bce, 0x56d4dd, 0xf2cc60, 0xff7b72,
        ],
    ),
    make(
        "ocean",
        "Océan",
        "Ocean",
        0x071a24,
        0x0b2634,
        0x103344,
        0x081e29,
        0x1d4a5e,
        0xd9f2ff,
        0x86b3c7,
        0x568196,
        0x2dd4bf,
        0x104f55,
        0x99f6e4,
        0x041318,
        0x6ee7b7,
        0xfde68a,
        0xfb7185,
        0xfbbf24,
        0xc4b5fd,
        0xf9a8d4,
        [
            0x5eead4, 0x93c5fd, 0xfde68a, 0xf9a8d4, 0xc4b5fd, 0x67e8f9, 0x86efac, 0xfca5a5,
        ],
    ),
    make(
        "foret",
        "Forêt",
        "Forest",
        0x0c1410,
        0x142019,
        0x1b2c22,
        0x0f1913,
        0x2c4636,
        0xdcefe3,
        0x8fb3a1,
        0x5f7d6d,
        0x6ee7a0,
        0x1b4d33,
        0xc7f9d8,
        0x07110b,
        0x86efac,
        0xfde68a,
        0xfca5a5,
        0xfdba74,
        0xd8b4fe,
        0xf9a8d4,
        [
            0x86efac, 0x93c5fd, 0xfde68a, 0xf9a8d4, 0xd8b4fe, 0x5eead4, 0xfca5a5, 0xfdba74,
        ],
    ),
    make(
        "amethyste",
        "Améthyste",
        "Amethyst",
        0x130f1c,
        0x1c1528,
        0x261c36,
        0x160f21,
        0x3d2d55,
        0xeee6ff,
        0xaa98c7,
        0x74638f,
        0xb388ff,
        0x3f2a66,
        0xe9d5ff,
        0x150f21,
        0x86efac,
        0xfde68a,
        0xfda4af,
        0xfbbf24,
        0xd8b4fe,
        0xf0abfc,
        [
            0xc4b5fd, 0xf0abfc, 0x93c5fd, 0x86efac, 0xfde68a, 0x5eead4, 0xfca5a5, 0xfdba74,
        ],
    ),
    make(
        "sakura",
        "Sakura",
        "Sakura",
        0x1a1016,
        0x241820,
        0x301f2b,
        0x1e1219,
        0x4a2b3d,
        0xffe4ef,
        0xc79aae,
        0x8e6a7c,
        0xff8fab,
        0x5c293c,
        0xffd6e2,
        0x1a1016,
        0x86efac,
        0xfde68a,
        0xff7b72,
        0xfbbf24,
        0xe9b8ff,
        0xffb3c8,
        [
            0xffb3c8, 0xf0abfc, 0xffd6e2, 0xfda4af, 0xc4b5fd, 0x93c5fd, 0xfde68a, 0x86efac,
        ],
    ),
    make(
        "dracula",
        "Dracula",
        "Dracula",
        0x282a36,
        0x303341,
        0x383b4b,
        0x22242f,
        0x4b4f63,
        0xf8f8f2,
        0xb8b9c9,
        0x7d7f93,
        0xbd93f9,
        0x40355c,
        0xd7b8ff,
        0x282a36,
        0x50fa7b,
        0xf1fa8c,
        0xff5555,
        0xffb86c,
        0xff79c6,
        0xff92d0,
        [
            0xbd93f9, 0x8be9fd, 0x50fa7b, 0xf1fa8c, 0xff79c6, 0xffb86c, 0xff5555, 0xa4ffff,
        ],
    ),
    make(
        "nord",
        "Nord",
        "Nord",
        0x2e3440,
        0x3b4252,
        0x434c5e,
        0x2b303b,
        0x4c566a,
        0xeceff4,
        0xa3abbc,
        0x6d7689,
        0x88c0d0,
        0x35515c,
        0xb8e3ee,
        0x2e3440,
        0xa3be8c,
        0xebcb8b,
        0xbf616a,
        0xd08770,
        0xb48ead,
        0xd08770,
        [
            0x88c0d0, 0x81a1c1, 0xa3be8c, 0xebcb8b, 0xb48ead, 0x8fbcbb, 0xd08770, 0xbf616a,
        ],
    ),
    make(
        "gruvbox",
        "Gruvbox",
        "Gruvbox",
        0x282828,
        0x32302f,
        0x3c3836,
        0x1d2021,
        0x504945,
        0xebdbb2,
        0xa89984,
        0x7c6f64,
        0xfabd2f,
        0x5a4a1f,
        0xffd75f,
        0x282828,
        0xb8bb26,
        0xfabd2f,
        0xfb4934,
        0xfe8019,
        0xd3869b,
        0xd3869b,
        [
            0x83a598, 0x8ec07c, 0xfabd2f, 0xd3869b, 0xd79921, 0x458588, 0xfb4934, 0xfe8019,
        ],
    ),
    make(
        "monokai",
        "Monokai",
        "Monokai",
        0x272822,
        0x2f312a,
        0x3a3c33,
        0x1f201b,
        0x4b4d43,
        0xf8f8f2,
        0xb3b5a9,
        0x7c7f73,
        0xa6e22e,
        0x3b5015,
        0xd0f58a,
        0x272822,
        0xa6e22e,
        0xe6db74,
        0xf92672,
        0xfd971f,
        0xae81ff,
        0xf92672,
        [
            0xa6e22e, 0x66d9ef, 0xe6db74, 0xf92672, 0xae81ff, 0xfd971f, 0x75715e, 0xf8f8f2,
        ],
    ),
    make(
        "solarized",
        "Solarized sombre",
        "Solarized Dark",
        0x002b36,
        0x073642,
        0x0b4350,
        0x00212b,
        0x184f5e,
        0xeee8d5,
        0x93a1a1,
        0x657b83,
        0x2aa198,
        0x0b4b48,
        0x6fd8ce,
        0x002b36,
        0x859900,
        0xb58900,
        0xdc322f,
        0xcb4b16,
        0x6c71c4,
        0xd33682,
        [
            0x2aa198, 0x268bd2, 0x859900, 0xb58900, 0x6c71c4, 0xcb4b16, 0xd33682, 0x93a1a1,
        ],
    ),
    make(
        "minuit",
        "Minuit",
        "Midnight",
        0x050608,
        0x0b0d11,
        0x11151b,
        0x080a0d,
        0x1e252e,
        0xc9d1d9,
        0x7d8794,
        0x4f5966,
        0x4f9cf9,
        0x17314f,
        0x9ecbff,
        0x050608,
        0x56d364,
        0xe3b341,
        0xf85149,
        0xf0883e,
        0xbc8cff,
        0xff7ab6,
        [
            0x79c0ff, 0x56d364, 0xe3b341, 0xff7ab6, 0xbc8cff, 0x39c5cf, 0xf0883e, 0xf85149,
        ],
    ),
    make(
        "carbone",
        "Carbone",
        "Carbon",
        0x161616,
        0x1f1f1f,
        0x292929,
        0x101010,
        0x393939,
        0xf2f2f2,
        0xa8a8a8,
        0x6e6e6e,
        0xff8a3d,
        0x5a2d0f,
        0xffb27a,
        0x161616,
        0x7ee787,
        0xf2cc60,
        0xff6b6b,
        0xffa657,
        0xd2a8ff,
        0xff9bce,
        [
            0xffa657, 0x79c0ff, 0x7ee787, 0xf2cc60, 0xd2a8ff, 0x56d4dd, 0xff9bce, 0xff6b6b,
        ],
    ),
    make(
        "cyberpunk",
        "Cyberpunk",
        "Cyberpunk",
        0x0a0014,
        0x14001f,
        0x1e002e,
        0x0d001a,
        0x3d0a52,
        0xf7e8ff,
        0xc39ddb,
        0x8a5aa8,
        0xff2a6d,
        0x5c0b2a,
        0xff7aa8,
        0x0a0014,
        0x00ffa3,
        0xfff700,
        0xff2a6d,
        0xff9f1c,
        0x9d4edd,
        0xff6ec7,
        [
            0xff6ec7, 0x00ffa3, 0xfff700, 0x9d4edd, 0xff9f1c, 0x00d9ff, 0xff2a6d, 0xc77dff,
        ],
    ),
    make(
        "papier",
        "Papier",
        "Paper",
        0xf6f1e7,
        0xece5d8,
        0xe2d9c8,
        0xfffaf2,
        0xcfc3ae,
        0x2f2a24,
        0x6c6458,
        0x948a7b,
        0x9a5b2e,
        0xe6d3be,
        0x7a431f,
        0xfffaf2,
        0x3f7d3f,
        0x9a7b1f,
        0xb33939,
        0xa85f1a,
        0x6b4fa0,
        0xa84874,
        [
            0x8a4b1f, 0x3f6d8a, 0x3f7d3f, 0x9a7b1f, 0x6b4fa0, 0x1f7a7a, 0xa84874, 0xb33939,
        ],
    ),
    make(
        "solarized-light",
        "Solarized clair",
        "Solarized Light",
        0xfdf6e3,
        0xeee8d5,
        0xe4ddc8,
        0xfffbf0,
        0xcfc8b4,
        0x073642,
        0x586e75,
        0x839496,
        0x268bd2,
        0xd6e6f2,
        0x1a5f91,
        0xfdf6e3,
        0x859900,
        0xb58900,
        0xdc322f,
        0xcb4b16,
        0x6c71c4,
        0xd33682,
        [
            0x268bd2, 0x2aa198, 0x859900, 0xb58900, 0x6c71c4, 0xcb4b16, 0xd33682, 0x586e75,
        ],
    ),
    make(
        "glacier",
        "Glacier",
        "Glacier",
        0xeef6fb,
        0xdeebf5,
        0xd0e1ee,
        0xf9fcff,
        0xb7cddd,
        0x17313f,
        0x4f6b7d,
        0x7893a5,
        0x0b7fb3,
        0xc5e3f2,
        0x086087,
        0xf9fcff,
        0x2f7d4f,
        0x9a7b1f,
        0xb33939,
        0xa85f1a,
        0x6b4fa0,
        0xa84874,
        [
            0x0b6e99, 0x2f7d4f, 0x8a6d1f, 0x6b4fa0, 0xa84874, 0x1f7a7a, 0xb33939, 0xa85f1a,
        ],
    ),
    make(
        "rose",
        "Rosé",
        "Rosé",
        0xfdf2f5,
        0xf7e5eb,
        0xefd6df,
        0xfff9fb,
        0xd9b8c4,
        0x3b2430,
        0x7d5a68,
        0xa17f8d,
        0xb84a76,
        0xf0d3df,
        0x933a5e,
        0xfff9fb,
        0x3f7d4f,
        0x9a7b1f,
        0xb33939,
        0xa85f1a,
        0x6b4fa0,
        0xb84a76,
        [
            0xa84874, 0x8a4b6b, 0x3f7d4f, 0x8a6d1f, 0x6b4fa0, 0x1f7a7a, 0xb33939, 0xa85f1a,
        ],
    ),
    make(
        "terminal",
        "Terminal",
        "Terminal",
        0x000000,
        0x0a0f0a,
        0x101810,
        0x050805,
        0x1f3a1f,
        0x8bff8b,
        0x5fbf5f,
        0x3d8a3d,
        0x00ff66,
        0x0a3d1b,
        0x99ffbb,
        0x000000,
        0x00ff66,
        0xffff66,
        0xff5555,
        0xffa500,
        0xcc99ff,
        0xff66cc,
        [
            0x00ff66, 0x66ffcc, 0xffff66, 0xff66cc, 0xcc99ff, 0x66ddff, 0xffa500, 0xff5555,
        ],
    ),
    make(
        "tux",
        "Tux (Linux)",
        "Tux (Linux)",
        0x0a0e14,
        0x111820,
        0x18222c,
        0x0d131a,
        0x2b3a46,
        0xe6edf3,
        0x93a4b5,
        0x5f7285,
        0xf5c542,
        0x3a2f0b,
        0xffd76a,
        0x0a0e14,
        0x7ee787,
        0xf5c542,
        0xff6b6b,
        0xffa657,
        0xc9a0ff,
        0xff9bce,
        [
            0x7ee787, 0x79c0ff, 0xf5c542, 0xff9bce, 0xc9a0ff, 0x56d4dd, 0xffa657, 0xff6b6b,
        ],
    ),
];

#[derive(Serialize, Deserialize)]
struct ThemeFile {
    key: String,
    name_fr: String,
    name_en: String,
    bg: u32,
    panel: u32,
    panel_alt: u32,
    input_bg: u32,
    border: u32,
    text: u32,
    muted: u32,
    faint: u32,
    accent: u32,
    accent_dark: u32,
    accent_hover: u32,
    on_accent: u32,
    green: u32,
    yellow: u32,
    red: u32,
    orange: u32,
    purple: u32,
    pink: u32,
    nicks: [u32; 8],
}

impl ThemeFile {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            key: theme.key.to_string(),
            name_fr: theme.name_fr.to_string(),
            name_en: theme.name_en.to_string(),
            bg: theme.bg,
            panel: theme.panel,
            panel_alt: theme.panel_alt,
            input_bg: theme.input_bg,
            border: theme.border,
            text: theme.text,
            muted: theme.muted,
            faint: theme.faint,
            accent: theme.accent,
            accent_dark: theme.accent_dark,
            accent_hover: theme.accent_hover,
            on_accent: theme.on_accent,
            green: theme.green,
            yellow: theme.yellow,
            red: theme.red,
            orange: theme.orange,
            purple: theme.purple,
            pink: theme.pink,
            nicks: theme.nicks,
        }
    }

    fn into_theme(self) -> Theme {
        let leak = |value: String| Box::leak(value.into_boxed_str()) as &'static str;
        Theme {
            key: leak(self.key),
            name_fr: leak(self.name_fr),
            name_en: leak(self.name_en),
            bg: self.bg,
            panel: self.panel,
            panel_alt: self.panel_alt,
            input_bg: self.input_bg,
            border: self.border,
            text: self.text,
            muted: self.muted,
            faint: self.faint,
            accent: self.accent,
            accent_dark: self.accent_dark,
            accent_hover: self.accent_hover,
            on_accent: self.on_accent,
            green: self.green,
            yellow: self.yellow,
            red: self.red,
            orange: self.orange,
            purple: self.purple,
            pink: self.pink,
            nicks: self.nicks,
        }
    }
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static ALL_THEMES: OnceLock<Mutex<&'static [Theme]>> = OnceLock::new();

fn all_themes() -> &'static Mutex<&'static [Theme]> {
    ALL_THEMES.get_or_init(|| Mutex::new(THEMES))
}

pub fn themes() -> &'static [Theme] {
    *all_themes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn current() -> &'static Theme {
    let themes = themes();
    &themes[CURRENT.load(Ordering::Relaxed).min(themes.len() - 1)]
}

pub fn current_index() -> usize {
    CURRENT
        .load(Ordering::Relaxed)
        .min(themes().len().saturating_sub(1))
}

pub fn set_theme(index: usize) {
    if index < themes().len() {
        CURRENT.store(index, Ordering::Relaxed);
    }
}

pub fn set_theme_key(key: &str) -> usize {
    if let Some(index) = themes().iter().position(|theme| theme.key == key) {
        set_theme(index);
        index
    } else {
        current_index()
    }
}

fn custom_theme_path() -> PathBuf {
    ircv3_core::Config::path()
        .parent()
        .map(|parent| parent.join("custom-theme.json"))
        .unwrap_or_else(|| PathBuf::from("custom-theme.json"))
}

pub fn export_current_json() -> String {
    serde_json::to_string_pretty(&ThemeFile::from_theme(current()))
        .unwrap_or_else(|_| "{}".to_string())
}

pub fn import_json(text: &str) -> Result<usize, String> {
    let mut file: ThemeFile =
        serde_json::from_str(text).map_err(|error| format!("thème invalide : {error}"))?;
    if file.key.trim().is_empty() {
        file.key = "custom".into();
    }
    let mut key = file.key.trim().to_string();
    let themes = themes();
    if themes.iter().any(|theme| theme.key == key) {
        let base = key.clone();
        let mut suffix = 2;
        while themes.iter().any(|theme| theme.key == key) {
            key = format!("{base}-{suffix}");
            suffix += 1;
        }
    }
    file.key = key;
    if file.name_fr.trim().is_empty() {
        file.name_fr = file.key.clone();
    }
    if file.name_en.trim().is_empty() {
        file.name_en = file.name_fr.clone();
    }
    let theme = file.into_theme();
    let index = themes.len();
    let mut next = themes.to_vec();
    next.push(theme);
    let leaked: &'static [Theme] = Box::leak(next.into_boxed_slice());
    *all_themes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = leaked;
    if let Some(parent) = custom_theme_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = fs::write(custom_theme_path(), export_index_json(index)) {
        log::warn!("sauvegarde du thème importé impossible : {error}");
    }
    Ok(index)
}

fn export_index_json(index: usize) -> String {
    let themes = themes();
    serde_json::to_string_pretty(&ThemeFile::from_theme(&themes[index])).unwrap_or_default()
}

pub fn load_custom_theme() {
    if let Ok(text) = fs::read_to_string(custom_theme_path()) {
        let _ = import_json(&text);
    }
}

pub fn bg() -> Hsla {
    rgb(current().bg).into()
}
pub fn panel() -> Hsla {
    rgb(current().panel).into()
}
pub fn panel_alt() -> Hsla {
    rgb(current().panel_alt).into()
}
pub fn input_bg() -> Hsla {
    rgb(current().input_bg).into()
}
pub fn border() -> Hsla {
    rgb(current().border).into()
}
pub fn text() -> Hsla {
    rgb(current().text).into()
}
pub fn muted() -> Hsla {
    rgb(current().muted).into()
}
pub fn faint() -> Hsla {
    rgb(current().faint).into()
}
pub fn accent() -> Hsla {
    rgb(current().accent).into()
}
pub fn accent_dark() -> Hsla {
    rgb(current().accent_dark).into()
}
pub fn accent_hover() -> Hsla {
    rgb(current().accent_hover).into()
}
pub fn on_accent() -> Hsla {
    rgb(current().on_accent).into()
}
pub fn green() -> Hsla {
    rgb(current().green).into()
}
pub fn yellow() -> Hsla {
    rgb(current().yellow).into()
}
pub fn red() -> Hsla {
    rgb(current().red).into()
}
pub fn orange() -> Hsla {
    rgb(current().orange).into()
}
pub fn purple() -> Hsla {
    rgb(current().purple).into()
}
pub fn pink() -> Hsla {
    rgb(current().pink).into()
}
pub fn accent_alpha(alpha: f32) -> Hsla {
    rgba((current().accent << 8) | ((alpha.clamp(0.0, 1.0) * 255.0) as u32)).into()
}
pub fn error_alpha(alpha: f32) -> Hsla {
    rgba((current().red << 8) | ((alpha.clamp(0.0, 1.0) * 255.0) as u32)).into()
}

#[cfg(target_os = "linux")]
pub const UI_FONT: &str = "Noto Sans";
#[cfg(target_os = "macos")]
pub const UI_FONT: &str = ".SystemUIFont";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const UI_FONT: &str = ".SystemUIFont";

#[cfg(target_os = "linux")]
pub const EMOJI_FONT: &str = "Noto Color Emoji";
#[cfg(target_os = "macos")]
pub const EMOJI_FONT: &str = "Apple Color Emoji";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const EMOJI_FONT: &str = "Noto Color Emoji";

#[cfg(target_os = "linux")]
pub const MONO_FONT: &str = "Noto Sans Mono";
#[cfg(target_os = "macos")]
pub const MONO_FONT: &str = "Menlo";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub const MONO_FONT: &str = "monospace";

pub fn nick_color(nick: &str) -> Hsla {
    let theme = current();
    let mut hash = 2166136261u32;
    for byte in nick.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    rgb(theme.nicks[(hash as usize) % theme.nicks.len()]).into()
}

pub fn status_color(status: ConnectionStatus) -> Hsla {
    match status {
        ConnectionStatus::Connected => green(),
        ConnectionStatus::Connecting | ConnectionStatus::Registering => yellow(),
        ConnectionStatus::Disconnected => red(),
    }
}

pub fn line_color(kind: LineKind) -> Hsla {
    match kind {
        LineKind::Message => text(),
        LineKind::History => muted(),
        LineKind::Action => green(),
        LineKind::Notice => yellow(),
        LineKind::Error => red(),
        LineKind::Join => green(),
        LineKind::Part | LineKind::Quit => orange(),
        LineKind::Nick => purple(),
        LineKind::Mode => purple(),
        LineKind::Topic => accent(),
        LineKind::Kick => red(),
        LineKind::Invite => pink(),
        LineKind::Away => faint(),
        LineKind::Account => faint(),
        LineKind::Server => muted(),
        LineKind::Motd => faint(),
        LineKind::Info => muted(),
        LineKind::Other => muted(),
    }
}
