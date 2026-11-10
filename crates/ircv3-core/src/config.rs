//! Configuration persistante (`~/.config/le-salon/config.toml`).
//!
//! Le mot de passe SASL n'est mémorisé que si l'utilisateur coche
//! « mémoriser ». Il est alors confié au trousseau système (Secret Service,
//! Keychain ou Credential Manager) et n'apparaît plus dans le fichier. Si
//! aucun trousseau n'est joignable, le fichier sert de repli avec des
//! permissions 0600.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{irc_fold, secrets};

#[derive(Clone, Serialize, Deserialize)]
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
    /// Toasts affichés dans la fenêtre (2 à 5 secondes).
    pub in_app_toasts: bool,
    /// Durée d'affichage d'un toast, en secondes (1 à 10).
    pub toast_seconds: u64,
    /// Demander une confirmation avant un upload litterbox public.
    pub confirm_upload: bool,
    /// Ne notifier que les mentions et messages privés.
    pub notify_mentions_only: bool,
    /// Jouer un son lors d'une notification.
    pub sound_notifications: bool,
    /// Début et fin des heures calmes au format HH:MM (vides = désactivé).
    pub quiet_hours_start: String,
    pub quiet_hours_end: String,
    /// Clés des tampons dont les notifications sont coupées.
    pub muted_buffers: Vec<String>,
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
            in_app_toasts: true,
            toast_seconds: 3,
            confirm_upload: true,
            notify_mentions_only: true,
            sound_notifications: true,
            quiet_hours_start: String::new(),
            quiet_hours_end: String::new(),
            muted_buffers: Vec::new(),
            sidebar_width: 236.0,
            nick_width: 168.0,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("nick", &self.nick)
            .field("user", &self.user)
            .field("realname", &self.realname)
            .field("server", &self.server)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("verify_tls", &self.verify_tls)
            .field("sasl_account", &self.sasl_account)
            .field(
                "sasl_password",
                &if self.sasl_password.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("remember_sasl_password", &self.remember_sasl_password)
            .field("channels", &self.channels)
            .field("auto_reconnect", &self.auto_reconnect)
            .field("connect_on_start", &self.connect_on_start)
            .field("language", &self.language)
            .field("theme", &self.theme)
            .field("notifications", &self.notifications)
            .field("in_app_toasts", &self.in_app_toasts)
            .field("toast_seconds", &self.toast_seconds)
            .field("confirm_upload", &self.confirm_upload)
            .field("notify_mentions_only", &self.notify_mentions_only)
            .field("sound_notifications", &self.sound_notifications)
            .field("quiet_hours_start", &self.quiet_hours_start)
            .field("quiet_hours_end", &self.quiet_hours_end)
            .field("muted_buffers", &self.muted_buffers.len())
            .field("sidebar_width", &self.sidebar_width)
            .field("nick_width", &self.nick_width)
            .finish()
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
                        let _ = Self::write_text(&path, &text);
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
        let host = self.server.clone();
        let account = self.sasl_account.clone();
        if !self.remember_sasl_password {
            self.sasl_password.clear();
            // Efface l'entrée de ce réseau, sans toucher aux autres.
            if let Err(error) = secrets::forget(&host, &account) {
                log::debug!("suppression du mot de passe SASL impossible : {error}");
            }
            return;
        }
        if !self.sasl_password.is_empty() {
            // Ancienne configuration (ou repli) : on déplace le secret vers
            // le trousseau, puis on réécrit le fichier sans le mot de passe.
            match secrets::store(&host, &account, &self.sasl_password) {
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
        match secrets::get(&host, &account) {
            Ok(Some(password)) => self.sasl_password = password,
            Ok(None) => {
                // Migration de l'ancienne entrée unique (≤ 1.6.0).
                match secrets::get_legacy() {
                    Ok(Some(password)) => {
                        if secrets::store(&host, &account, &password).is_ok() {
                            let _ = secrets::forget_legacy();
                        }
                        self.sasl_password = password;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        log::debug!("lecture du trousseau système impossible : {error}")
                    }
                }
            }
            Err(error) => log::debug!("lecture du trousseau système impossible : {error}"),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut saved = self.clone();
        if saved.remember_sasl_password {
            if !saved.sasl_password.is_empty() {
                match secrets::store(&saved.server, &saved.sasl_account, &saved.sasl_password) {
                    // Le secret est dans le trousseau : il ne doit pas
                    // apparaître dans le fichier.
                    Ok(()) => saved.sasl_password.clear(),
                    Err(error) => Self::warn_plaintext_fallback(&error),
                }
            } else {
                // Un mot de passe volontairement vidé ne doit pas ressusciter
                // depuis le trousseau au prochain lancement.
                if let Err(error) = secrets::forget(&saved.server, &saved.sasl_account) {
                    log::debug!("suppression du mot de passe SASL impossible : {error}");
                }
            }
        } else {
            if let Err(error) = secrets::forget(&saved.server, &saved.sasl_account) {
                log::debug!("suppression du mot de passe SASL impossible : {error}");
            }
            saved.sasl_password.clear();
        }
        Self::write(&Self::path(), &saved)
    }

    /// Sauvegarde sans jamais toucher au trousseau ni écrire de secret.
    /// Utilisé pour les connexions lancées en ligne de commande : un
    /// `--sasl-password` ponctuel ne doit pas être mémorisé.
    pub fn save_without_secret(&self) -> std::io::Result<()> {
        let mut saved = self.clone();
        saved.sasl_password.clear();
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
        let text = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
        Self::write_text(path, &text)
    }

    /// Écrit un fichier privé en s'assurant que le dossier est en 0700 et que
    /// le fichier est créé en 0600, sans passer par un fichier temporaire
    /// world-readable.
    fn write_text(path: &Path, text: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt;

            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(path)?;
            file.write_all(text.as_bytes())?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        {
            fs::write(path, text)?;
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

    /// Construit une connexion sans réutiliser le mot de passe mémorisé.
    /// L'appelant peut ensuite charger le secret du réseau demandé.
    pub fn connection_without_secret(&self) -> ConnectionConfig {
        self.connection(Some(String::new()))
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

#[derive(Clone)]
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

impl fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("verify_tls", &self.verify_tls)
            .field("nick", &self.nick)
            .field("user", &self.user)
            .field("realname", &self.realname)
            .field("sasl_account", &self.sasl_account)
            .field(
                "sasl_password",
                &if self.sasl_password.is_some() {
                    "<redacted>"
                } else {
                    "<none>"
                },
            )
            .field("channels", &self.channels)
            .field("auto_reconnect", &self.auto_reconnect)
            .finish()
    }
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

    /// Refuse les combinaisons qui exposeraient le mot de passe SASL.
    ///
    /// Le mot de passe SASL PLAIN doit toujours circuler dans un tunnel TLS
    /// dont le certificat est vérifié. La vérification est une défense
    /// indispensable contre un MITM actif : sans elle, TLS ne protège pas
    /// contre une attaque de l'homme du milieu.
    pub fn security_check(&self) -> Result<(), String> {
        if self.has_sasl() && !self.tls {
            return Err(crate::i18n::t(
                "SASL sans TLS est interdit : le mot de passe serait envoyé en clair. Activez TLS ou videz le compte SASL.",
                "SASL without TLS is not allowed: the password would be sent in clear text. Enable TLS or clear the SASL account.",
            )
            .into());
        }
        if self.has_sasl() && !self.verify_tls {
            return Err(crate::i18n::t(
                "SASL avec un certificat non vérifié est interdit : un MITM pourrait voler le mot de passe. Activez la vérification du certificat.",
                "SASL with certificate verification disabled is not allowed: a MITM could steal the password. Enable certificate verification.",
            )
            .into());
        }
        Ok(())
    }

    pub fn normalized(mut self) -> Self {
        self.host = self.host.trim().to_string();
        // `ircs://` ne peut que renforcer la sécurité. `irc://` ne doit
        // jamais désactiver TLS dans le dos de l'utilisateur : c'est la case
        // « Connexion TLS » qui fait foi.
        if self
            .host
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ircs://"))
        {
            self.host = self.host[7..].trim_end_matches('/').to_string();
            self.tls = true;
        } else if self
            .host
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("irc://"))
        {
            self.host = self.host[6..].trim_end_matches('/').to_string();
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

    fn connection(tls: bool, verify_tls: bool, sasl_account: Option<&str>) -> ConnectionConfig {
        ConnectionConfig {
            host: "irc.example.org".into(),
            port: 6697,
            tls,
            verify_tls,
            nick: "moi".into(),
            user: "moi".into(),
            realname: "Moi".into(),
            sasl_account: sasl_account.map(str::to_string),
            sasl_password: sasl_account.map(|_| "secret".to_string()),
            channels: Vec::new(),
            auto_reconnect: true,
        }
    }

    #[test]
    fn sasl_requires_tls_and_certificate_verification() {
        assert!(connection(true, true, Some("moi")).security_check().is_ok());
        assert!(connection(true, true, None).security_check().is_ok());
        assert!(connection(true, false, None).security_check().is_ok());
        assert!(
            connection(false, true, Some("moi"))
                .security_check()
                .is_err()
        );
        assert!(
            connection(true, false, Some("moi"))
                .security_check()
                .is_err()
        );
        assert!(
            connection(false, false, Some("moi"))
                .security_check()
                .is_err()
        );
    }

    #[test]
    fn irc_scheme_does_not_disable_tls() {
        let unchanged = ConnectionConfig {
            host: "irc://irc.example.org".into(),
            tls: true,
            ..connection(true, true, None)
        }
        .normalized();
        assert_eq!(unchanged.host, "irc.example.org");
        assert!(unchanged.tls);

        let explicit_plain = ConnectionConfig {
            host: "irc://irc.example.org".into(),
            tls: false,
            ..connection(false, true, None)
        }
        .normalized();
        assert!(!explicit_plain.tls);
    }

    #[test]
    fn ircs_scheme_forces_tls() {
        let secure = ConnectionConfig {
            host: "IRCS://irc.example.org/".into(),
            tls: false,
            ..connection(false, true, None)
        }
        .normalized();
        assert_eq!(secure.host, "irc.example.org");
        assert!(secure.tls);
    }

    #[test]
    fn missing_preference_fields_use_secure_defaults() {
        let config: Config = toml::from_str("nick = \"alice\"\n").unwrap();
        assert!(config.in_app_toasts);
        assert_eq!(config.toast_seconds, 3);
        assert!(config.confirm_upload);
        assert!(config.notify_mentions_only);
        assert!(config.tls);
        assert!(config.verify_tls);
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
