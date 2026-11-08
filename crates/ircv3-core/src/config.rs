//! Configuration persistante (`~/.config/le-salon/config.toml`).
//!
//! Le mot de passe SASL n'est mémorisé que si l'utilisateur coche
//! « mémoriser ». Il est alors confié au trousseau système (Secret Service,
//! Keychain ou Credential Manager) et n'apparaît plus dans le fichier. Si
//! aucun trousseau n'est joignable, le fichier sert de repli avec des
//! permissions 0600.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{irc_fold, secrets};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub nick: String,
    pub user: String,
    pub realname: String,
    pub server: String,
    pub port: u16,
    pub tls: bool,
    pub verify_tls: bool,
    pub sasl_account: String,
    /// Mot de passe SASL en mémoire. Il est écrit dans le trousseau système ;
    /// le fichier (0600) ne le reçoit qu'en repli si aucun trousseau n'est
    /// disponible.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sasl_password: String,
    pub remember_sasl_password: bool,
    pub channels: String,
    pub auto_reconnect: bool,
    pub connect_on_start: bool,
    /// "auto", "fr" ou "en".
    pub language: String,
    /// Clé du thème (voir `theme::THEMES`).
    pub theme: String,
    pub notifications: bool,
    pub sidebar_width: f32,
    pub nick_width: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nick: "lesalon".into(),
            user: "lesalon".into(),
            realname: "Le Salon".into(),
            server: "irc.libera.chat".into(),
            port: 6697,
            tls: true,
            verify_tls: true,
            sasl_account: String::new(),
            sasl_password: String::new(),
            remember_sasl_password: true,
            channels: String::new(),
            auto_reconnect: true,
            connect_on_start: false,
            language: "auto".into(),
            theme: "nuit".into(),
            notifications: true,
            sidebar_width: 236.0,
            nick_width: 168.0,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("le-salon")
            .join("config.toml")
    }

    pub fn legacy_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from(".config"))
            .join("ircv3-gpui")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                let legacy = Self::legacy_path();
                match fs::read_to_string(&legacy) {
                    Ok(text) => {
                        log::info!(
                            "migration de la configuration {} -> {}",
                            legacy.display(),
                            path.display()
                        );
                        if let Some(parent) = path.parent() {
                            let _ = fs::create_dir_all(parent);
                        }
                        let _ = fs::write(&path, &text);
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
                        }
                        text
                    }
                    Err(error) => {
                        log::debug!("configuration absente ({}): {error}", path.display());
                        return Self::default();
                    }
                }
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(mut config) => {
                config.restore_secret();
                config
            }
            Err(error) => {
                log::warn!("configuration invalide ({}): {error}", path.display());
                Self::default()
            }
        }
    }

    /// Recharge le mot de passe depuis le trousseau système, ou migre un
    /// éventuel mot de passe encore présent en clair dans le fichier.
    fn restore_secret(&mut self) {
        if !self.remember_sasl_password {
            self.sasl_password.clear();
            // Efface une entrée laissée par une session précédente.
            if let Err(error) = secrets::forget() {
                log::debug!("suppression du mot de passe SASL impossible : {error}");
            }
            return;
        }
        if !self.sasl_password.is_empty() {
            // Ancienne configuration (ou repli) : on déplace le secret vers
            // le trousseau, puis on réécrit le fichier sans le mot de passe.
            match secrets::store(&self.sasl_password) {
                Ok(()) => {
                    log::info!("mot de passe SASL migré vers le trousseau système");
                    let mut flushed = self.clone();
                    flushed.sasl_password.clear();
                    if let Err(error) = Self::write(&Self::path(), &flushed) {
                        log::warn!("réécriture de la configuration impossible : {error}");
                    }
                }
                Err(error) => Self::warn_plaintext_fallback(&error),
            }
            return;
        }
        match secrets::get() {
            Ok(Some(password)) => self.sasl_password = password,
            Ok(None) => {}
            Err(error) => log::debug!("lecture du trousseau système impossible : {error}"),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut saved = self.clone();
        if saved.remember_sasl_password {
            if !saved.sasl_password.is_empty() {
                match secrets::store(&saved.sasl_password) {
                    // Le secret est dans le trousseau : il ne doit pas
                    // apparaître dans le fichier.
                    Ok(()) => saved.sasl_password.clear(),
                    Err(error) => Self::warn_plaintext_fallback(&error),
                }
            }
        } else {
            if let Err(error) = secrets::forget() {
                log::debug!("suppression du mot de passe SASL impossible : {error}");
            }
            saved.sasl_password.clear();
        }
        Self::write(&Self::path(), &saved)
    }

    /// Écrit la configuration à un emplacement explicite, sans toucher au
    /// trousseau (utilisé par les tests). Le mot de passe reste alors dans le
    /// fichier, protégé par les permissions du système.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let mut saved = self.clone();
        if !saved.remember_sasl_password {
            saved.sasl_password.clear();
        }
        Self::write(path, &saved)
    }

    fn write(path: &Path, config: &Config) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }
        let text = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
        fs::write(path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn warn_plaintext_fallback(error: &keyring::Error) {
        static WARNED: AtomicBool = AtomicBool::new(false);
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "trousseau système indisponible ({error}) ; le mot de passe SASL est conservé dans {} (0600)",
                Self::path().display()
            );
        }
    }

    pub fn parsed_channels(&self) -> Vec<String> {
        parse_channels(&self.channels)
    }

    pub fn connection(&self, sasl_password: Option<String>) -> ConnectionConfig {
        let saved_password = (self.remember_sasl_password && !self.sasl_password.is_empty())
            .then(|| self.sasl_password.clone());
        let sasl_password = sasl_password.or(saved_password);
        ConnectionConfig {
            host: self.server.clone(),
            port: self.port,
            tls: self.tls,
            verify_tls: self.verify_tls,
            nick: self.nick.clone(),
            user: if self.user.is_empty() {
                self.nick.clone()
            } else {
                self.user.clone()
            },
            realname: if self.realname.is_empty() {
                self.nick.clone()
            } else {
                self.realname.clone()
            },
            sasl_account: (!self.sasl_account.trim().is_empty())
                .then(|| self.sasl_account.trim().to_string()),
            sasl_password: sasl_password.filter(|password| !password.is_empty()),
            channels: self.parsed_channels(),
            auto_reconnect: self.auto_reconnect,
        }
    }

    pub fn remember(&mut self, connection: &ConnectionConfig) {
        self.nick = connection.nick.clone();
        self.user = connection.user.clone();
        self.realname = connection.realname.clone();
        self.server = connection.host.clone();
        self.port = connection.port;
        self.tls = connection.tls;
        self.verify_tls = connection.verify_tls;
        self.sasl_account = connection.sasl_account.clone().unwrap_or_default();
        self.channels = connection.channels.join(",");
        self.auto_reconnect = connection.auto_reconnect;
    }
}

#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_tls: bool,
    pub nick: String,
    pub user: String,
    pub realname: String,
    pub sasl_account: Option<String>,
    pub sasl_password: Option<String>,
    pub channels: Vec<String>,
    pub auto_reconnect: bool,
}

impl ConnectionConfig {
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn has_sasl(&self) -> bool {
        self.sasl_account
            .as_ref()
            .is_some_and(|account| !account.is_empty())
    }

    pub fn normalized(mut self) -> Self {
        self.host = self.host.trim().to_string();
        if let Some(host) = self.host.strip_prefix("ircs://") {
            self.host = host.trim_end_matches('/').to_string();
            self.tls = true;
        } else if let Some(host) = self.host.strip_prefix("irc://") {
            self.host = host.trim_end_matches('/').to_string();
            self.tls = false;
        }
        if self.port == 0 {
            self.port = if self.tls { 6697 } else { 6667 };
        }
        self.nick = self.nick.trim().to_string();
        self.user = self.user.trim().to_string();
        self.realname = self.realname.trim().to_string();
        if self.nick.is_empty() {
            self.nick = "lesalon".into();
        }
        if self.user.is_empty() {
            self.user = self.nick.clone();
        }
        if self.realname.is_empty() {
            self.realname = self.nick.clone();
        }
        self.channels = parse_channels(&self.channels.join(","));
        self
    }
}

pub fn parse_channels(input: &str) -> Vec<String> {
    let mut channels: Vec<String> = Vec::new();
    for raw in input.split([',', ' ', '\t', '\n']) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let channel = if raw.starts_with(['#', '&', '+', '!']) {
            raw.to_string()
        } else {
            format!("#{raw}")
        };
        if !channels
            .iter()
            .any(|existing| irc_fold(existing) == irc_fold(&channel))
        {
            channels.push(channel);
        }
    }
    channels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_are_normalized_and_deduped() {
        assert_eq!(
            parse_channels("#a, b, #A, &c"),
            vec!["#a".to_string(), "#b".to_string(), "&c".to_string()]
        );
    }

    #[test]
    fn connection_normalizes_scheme_and_port() {
        let connection = ConnectionConfig {
            host: "ircs://irc.example.org/".into(),
            port: 0,
            tls: false,
            verify_tls: true,
            nick: " moi ".into(),
            user: String::new(),
            realname: String::new(),
            sasl_account: None,
            sasl_password: None,
            channels: vec!["test".into()],
            auto_reconnect: true,
        }
        .normalized();
        assert_eq!(connection.host, "irc.example.org");
        assert!(connection.tls);
        assert_eq!(connection.port, 6697);
        assert_eq!(connection.nick, "moi");
        assert_eq!(connection.user, "moi");
        assert_eq!(connection.channels, vec!["#test"]);
    }

    #[test]
    fn config_roundtrip_without_password() {
        let dir = std::env::temp_dir().join(format!(
            "ircv3-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("config.toml");
        let mut config = Config::default();
        config.nick = "alice".into();
        config.sasl_password = "s3cr3t".into();
        config.remember_sasl_password = false;
        config.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("alice"));
        assert!(!text.contains("s3cr3t"));
        let loaded: Config = toml::from_str(&text).unwrap();
        assert_eq!(loaded.nick, "alice");
        assert!(loaded.sasl_password.is_empty());
        config.remember_sasl_password = true;
        config.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("s3cr3t"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
