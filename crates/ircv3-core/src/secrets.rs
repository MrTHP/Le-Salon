//! Mot de passe SASL dans le trousseau de secrets du système.
//!
//! La crate `keyring` utilise le Secret Service (GNOME Keyring, KWallet,
//! KeePassXC…) sous Linux via D-Bus, le trousseau Keychain sous macOS et le
//! Credential Manager sous Windows. Aucun secret n'est alors écrit dans le
//! fichier de configuration.
//!
//! Les secrets sont indexés par couple `(hôte, compte)` : un mot de passe
//! mémorisé pour un réseau ne peut pas être réutilisé silencieusement pour un
//! autre. Si aucun trousseau n'est joignable (session sans D-Bus, conteneur,
//! machine distante…), les appelants retombent sur le fichier `config.toml`
//! en 0600 : [`crate::Config::save`] ne perd donc jamais le mot de passe.

use keyring::{Entry, Error};

/// Nom du service tel qu'il apparaît dans le trousseau.
const SERVICE: &str = "le-salon";
/// Ancienne entrée unique, conservée pour migrer les configurations 1.6.0.
const LEGACY_USER: &str = "sasl";

/// Encode une chaîne en hexadécimal pour obtenir une clé de trousseau stable,
/// sans caractère de contrôle ni ambiguïté entre hôte et compte.
fn encode(value: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(value.len() * 2);
    for byte in value.as_bytes() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn key(host: &str, account: &str) -> String {
    format!(
        "{}:{}",
        encode(&host.trim().to_ascii_lowercase()),
        encode(account.trim())
    )
}

fn entry_for(host: &str, account: &str) -> Result<Entry, Error> {
    Entry::new(SERVICE, &key(host, account))
}

fn legacy_entry() -> Result<Entry, Error> {
    Entry::new(SERVICE, LEGACY_USER)
}

/// Lit le mot de passe mémorisé pour ce réseau, ou `None` si absent.
pub fn get(host: &str, account: &str) -> Result<Option<String>, Error> {
    match entry_for(host, account)?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Enregistre (ou remplace) le mot de passe de ce réseau.
pub fn store(host: &str, account: &str, password: &str) -> Result<(), Error> {
    entry_for(host, account)?.set_password(password)
}

/// Supprime l'entrée de ce réseau. Une entrée déjà absente n'est pas une erreur.
pub fn forget(host: &str, account: &str) -> Result<(), Error> {
    match entry_for(host, account)?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Lit l'ancienne entrée unique `le-salon`/`sasl` (config ≤ 1.6.0).
pub fn get_legacy() -> Result<Option<String>, Error> {
    match legacy_entry()?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Supprime l'ancienne entrée unique après migration.
pub fn forget_legacy() -> Result<(), Error> {
    match legacy_entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(error),
    }
}
