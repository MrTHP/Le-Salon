//! Langue globale du socle et de l'interface (FR/EN).
//!
//! Le client GPUI détecte la langue du système au démarrage, puis appelle
//! [`set_lang`]. Les messages locaux du protocole et de l'interface utilisent
//! [`t`] pour rester cohérents dans la langue choisie.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Lang {
    Fr = 0,
    En = 1,
}

static LANG: AtomicU8 = AtomicU8::new(Lang::Fr as u8);

impl Lang {
    pub fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" | "english" | "anglais" => Self::En,
            "fr" | "french" | "francais" | "français" => Self::Fr,
            _ => Self::detect(),
        }
    }

    pub fn detect() -> Self {
        let mut values = Vec::new();
        for key in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            if let Ok(value) = std::env::var(key) {
                values.push(value);
            }
        }
        if let Some(locale) = sys_locale::get_locale() {
            values.push(locale);
        }
        for value in values {
            let value = value.trim().to_ascii_lowercase();
            if value.starts_with("fr") {
                return Self::Fr;
            }
            if value.starts_with("en") {
                return Self::En;
            }
        }
        Self::En
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::Fr => "fr",
            Self::En => "en",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fr => "Français",
            Self::En => "English",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Self::Fr => Self::En,
            Self::En => Self::Fr,
        }
    }
}

pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if LANG.load(Ordering::Relaxed) == Lang::En as u8 {
        Lang::En
    } else {
        Lang::Fr
    }
}

pub fn t(fr: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::Fr => fr,
        Lang::En => en,
    }
}
