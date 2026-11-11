//! Chiffrement IRC compatible avec le format RelayCrypt v1.
//!
//! Format filaire : `+relaycrypt:v1:<base64url(nonce || ciphertext || tag)>`.
//! XChaCha20-Poly1305 avec une clé de 32 octets dérivée par Argon2id pour les
//! phrases de passe. Les clés sont stockées localement en 0600.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::irc_fold;

const PREFIX: &str = "+relaycrypt:v1:";

#[derive(Clone, Debug)]
pub struct EncryptionStore {
    keys: BTreeMap<String, [u8; 32]>,
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct StoredKeys(BTreeMap<String, String>);

impl Default for EncryptionStore {
    fn default() -> Self {
        let path = crate::Config::path()
            .parent()
            .map(|parent| parent.join("encryption.json"))
            .unwrap_or_else(|| PathBuf::from("encryption.json"));
        Self::new(path)
    }
}

impl EncryptionStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let keys = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<StoredKeys>(&text).ok())
            .map(|stored| {
                stored
                    .0
                    .into_iter()
                    .filter_map(|(target, key)| {
                        let bytes = URL_SAFE_NO_PAD.decode(key).ok()?;
                        let key: [u8; 32] = bytes.try_into().ok()?;
                        Some((irc_fold(&target), key))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { keys, path }
    }
    pub fn key_for(&self, target: &str) -> Option<[u8; 32]> {
        self.keys.get(&irc_fold(target)).copied()
    }

    pub fn generate_key(&mut self, target: &str) -> String {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        self.set_bytes(target, key);
        URL_SAFE_NO_PAD.encode(key)
    }

    pub fn set_passphrase(&mut self, target: &str, passphrase: &str) -> Result<String, String> {
        let key = derive_key(passphrase, target)?;
        self.set_bytes(target, key);
        Ok(URL_SAFE_NO_PAD.encode(key))
    }

    pub fn set_base64(&mut self, target: &str, value: &str) -> Result<(), String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value.trim())
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value.trim()))
            .map_err(|_| "clé base64 invalide".to_string())?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "la clé doit faire 32 octets".to_string())?;
        self.set_bytes(target, key);
        Ok(())
    }

    pub fn remove(&mut self, target: &str) -> bool {
        let changed = self.keys.remove(&irc_fold(target)).is_some();
        if changed {
            self.save();
        }
        changed
    }

    pub fn targets(&self) -> Vec<String> {
        self.keys.keys().cloned().collect()
    }

    pub fn encrypt(&self, target: &str, text: &str) -> Option<String> {
        let key = self.key_for(target)?;
        encrypt_with_key(&key, text).ok()
    }

    pub fn decrypt(&self, target: &str, text: &str) -> Option<String> {
        if !text.starts_with(PREFIX) {
            return None;
        }
        let key = self.key_for(target)?;
        let payload = &text[PREFIX.len()..];
        decrypt_with_key(&key, payload).ok()
    }

    fn set_bytes(&mut self, target: &str, key: [u8; 32]) {
        self.keys.insert(irc_fold(target), key);
        self.save();
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
            restrict_dir(parent);
        }
        let stored = StoredKeys(
            self.keys
                .iter()
                .map(|(target, key)| (target.clone(), URL_SAFE_NO_PAD.encode(key)))
                .collect(),
        );
        if let Ok(text) = serde_json::to_string_pretty(&stored)
            && let Err(error) = write_private(&self.path, &text)
        {
            log::warn!("sauvegarde des clés de chiffrement impossible : {error}");
        }
    }
}

pub fn is_encrypted(text: &str) -> bool {
    text.starts_with(PREFIX)
}

pub fn derive_key(passphrase: &str, target: &str) -> Result<[u8; 32], String> {
    let salt = format!("le-salon-relaycrypt:{}", irc_fold(target));
    let params = Params::new(19_456, 2, 1, Some(32)).map_err(|error| error.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt.as_bytes(), &mut key)
        .map_err(|error| error.to_string())?;
    Ok(key)
}

fn encrypt_with_key(key: &[u8; 32], text: &str) -> Result<String, String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, text.as_bytes())
        .map_err(|error| error.to_string())?;
    let mut payload = Vec::with_capacity(24 + ciphertext.len());
    payload.extend_from_slice(nonce.as_slice());
    payload.extend_from_slice(&ciphertext);
    Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(payload)))
}

fn decrypt_with_key(key: &[u8; 32], payload: &str) -> Result<String, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|error| error.to_string())?;
    if bytes.len() < 24 {
        return Err("charge chiffrée trop courte".into());
    }
    let nonce = XNonce::from_slice(&bytes[..24]);
    let cipher = XChaCha20Poly1305::new(key.into());
    let plaintext = cipher
        .decrypt(nonce, &bytes[24..])
        .map_err(|error| error.to_string())?;
    String::from_utf8(plaintext).map_err(|error| error.to_string())
}

pub(crate) fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    let _ = path;
}

pub(crate) fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
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
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_roundtrip() {
        let key = derive_key("phrase secrète", "alice").unwrap();
        let encrypted = encrypt_with_key(&key, "bonjour\nmonde").unwrap();
        assert!(is_encrypted(&encrypted));
        let payload = encrypted.strip_prefix(PREFIX).unwrap();
        assert_eq!(decrypt_with_key(&key, payload).unwrap(), "bonjour\nmonde");
        assert!(decrypt_with_key(&key, "pas-base64").is_err());
    }
}
