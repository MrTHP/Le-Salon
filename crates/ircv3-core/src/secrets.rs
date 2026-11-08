//! Mot de passe SASL dans le trousseau de secrets du système.
//!
//! La crate `keyring` utilise le Secret Service (GNOME Keyring, KWallet,
//! KeePassXC…) sous Linux via D-Bus, le trousseau Keychain sous macOS et le
//! Credential Manager sous Windows. Aucun secret n'est alors écrit dans le
//! fichier de configuration.
//!
//! Si aucun trousseau n'est joignable (session sans D-Bus, conteneur, machine
//! distante…), les appelants retombent sur le fichier `config.toml` en 0600 :
//! [`crate::Config::save`] ne perd donc jamais le mot de passe.

use keyring::{Entry, Error};

/// Nom du service tel qu'il apparaît dans le trousseau.
const SERVICE: &str = "le-salon";
/// Une seule entrée : la configuration ne mémorise qu'un mot de passe SASL.
const USER: &str = "sasl";

fn entry() -> Result<Entry, Error> {
    Entry::new(SERVICE, USER)
}

/// Lit le mot de passe mémorisé, ou `None` si l'entrée n'existe pas.
pub fn get() -> Result<Option<String>, Error> {
    match entry()?.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Enregistre (ou remplace) le mot de passe mémorisé.
pub fn store(password: &str) -> Result<(), Error> {
    entry()?.set_password(password)
}

/// Supprime l'entrée du trousseau. Une entrée déjà absente n'est pas une erreur.
pub fn forget() -> Result<(), Error> {
    match entry()?.delete_credential() {
        Ok(()) | Err(Error::NoEntry) => Ok(()),
        Err(error) => Err(error),
    }
}
