//! Chiffrement IRC FiSH — mode de compatibilité.
//!
//! FiSH est le format historique utilisé par mIRC, HexChat, irssi, ZNC ou
//! Konversation pour chiffrer des messages avec Blowfish. Cette implémentation
//! ne couvre que le mode **ECB** (`+OK <base64 FiSH>`), celui que tous les
//! clients comprennent. Le mode CBC (`+OK *`) et l'échange de clés DH1080 ne
//! sont pas pris en charge.
//!
//! FiSH n'authentifie ni les messages ni les correspondants : ce n'est pas un
//! chiffrement moderne. Il est proposé uniquement pour parler aux clients qui
//! ne connaissent pas RelayCrypt.

use std::{collections::BTreeMap, fs, path::PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use blowfish::{
    Blowfish,
    cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
use serde::{Deserialize, Serialize};

use crate::irc_fold;

const PREFIX: &str = "+OK ";
const MCPS_PREFIX: &str = "mcps ";
const MIN_KEY: usize = 4;
const MAX_KEY: usize = 56;

/// Alphabet FiSH pour les charges ECB. Ce n'est pas du Base64 standard :
/// les blocs de 8 octets sont écrits en 12 caractères, mot de poids fort
/// d'abord dans un ordre particulier.
const FISH_B64: &[u8; 64] = b"./0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FishError {
    /// Aucune clé n'est configurée pour ce couple réseau/cible.
    NoKey,
    /// Message FiSH CBC : volontairement non pris en charge.
    UnsupportedMode,
    /// Charge base64 ou longueur de bloc invalide.
    InvalidCiphertext,
    /// Clé absente ou de longueur hors 4..=56 octets.
    InvalidKey,
    /// Le déchiffrement ne produit pas de l'UTF-8 (mauvaise clé probable).
    InvalidUtf8,
}

#[derive(Clone, Debug)]
pub struct FishStore {
    /// réseau (`hôte:port`) → cible (pseudo ou salon) → clé Blowfish.
    keys: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct StoredFish(BTreeMap<String, BTreeMap<String, String>>);

impl Default for FishStore {
    fn default() -> Self {
        let path = crate::Config::path()
            .parent()
            .map(|parent| parent.join("fish_keys.json"))
            .unwrap_or_else(|| PathBuf::from("fish_keys.json"));
        Self::new(path)
    }
}

impl FishStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let keys = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<StoredFish>(&text).ok())
            .map(|stored| {
                stored
                    .0
                    .into_iter()
                    .map(|(network, targets)| {
                        let network = normalize_network(&network);
                        let targets = targets
                            .into_iter()
                            .filter_map(|(target, encoded)| {
                                let bytes = STANDARD.decode(encoded).ok()?;
                                (MIN_KEY..=MAX_KEY)
                                    .contains(&bytes.len())
                                    .then(|| (irc_fold(&target), bytes))
                            })
                            .collect();
                        (network, targets)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { keys, path }
    }

    pub fn key_for(&self, network: &str, target: &str) -> Option<&[u8]> {
        self.keys
            .get(&normalize_network(network))
            .and_then(|targets| targets.get(&irc_fold(target)))
            .map(Vec::as_slice)
    }

    pub fn has_key(&self, network: &str, target: &str) -> bool {
        self.key_for(network, target).is_some()
    }

    pub fn set_text(&mut self, network: &str, target: &str, key: &str) -> Result<(), String> {
        self.set_bytes(network, target, key.as_bytes())
    }

    pub fn set_bytes(&mut self, network: &str, target: &str, key: &[u8]) -> Result<(), String> {
        if !(MIN_KEY..=MAX_KEY).contains(&key.len()) {
            return Err(format!(
                "la clé FiSH doit faire entre {MIN_KEY} et {MAX_KEY} octets"
            ));
        }
        self.keys
            .entry(normalize_network(network))
            .or_default()
            .insert(irc_fold(target), key.to_vec());
        self.save()
    }

    pub fn remove(&mut self, network: &str, target: &str) -> bool {
        let network = normalize_network(network);
        let changed = self
            .keys
            .get_mut(&network)
            .is_some_and(|targets| targets.remove(&irc_fold(target)).is_some());
        if changed && let Err(error) = self.save() {
            log::warn!("sauvegarde des clés FiSH impossible : {error}");
        }
        changed
    }

    pub fn targets(&self, network: &str) -> Vec<String> {
        self.keys
            .get(&normalize_network(network))
            .map(|targets| targets.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn encrypt(&self, network: &str, target: &str, text: &str) -> Option<String> {
        let key = self.key_for(network, target)?;
        encrypt_ecb(key, text).ok()
    }

    pub fn decrypt(&self, network: &str, target: &str, text: &str) -> Result<String, FishError> {
        if is_cbc(text) {
            return Err(FishError::UnsupportedMode);
        }
        let key = self.key_for(network, target).ok_or(FishError::NoKey)?;
        decrypt_ecb(key, text)
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            crate::crypto::restrict_dir(parent);
        }
        let stored = StoredFish(
            self.keys
                .iter()
                .map(|(network, targets)| {
                    let targets = targets
                        .iter()
                        .map(|(target, key)| (target.clone(), STANDARD.encode(key)))
                        .collect();
                    (network.clone(), targets)
                })
                .collect(),
        );
        let text = serde_json::to_string_pretty(&stored).map_err(|error| error.to_string())?;
        crate::crypto::write_private(&self.path, &text).map_err(|error| error.to_string())
    }
}

/// Clé de stockage stable d'un réseau : hôte normalisé et port.
pub fn network_key(host: &str, port: u16) -> String {
    format!("{}:{port}", host.trim().to_ascii_lowercase())
}

fn normalize_network(network: &str) -> String {
    network.trim().to_ascii_lowercase()
}

/// Vrai si le texte ressemble à un message FiSH pris en charge ou non.
pub fn is_encrypted(text: &str) -> bool {
    is_cbc(text) || ecb_payload(text).is_some_and(valid_payload)
}

/// Vrai pour les préfixes CBC de FiSH 10 / mIRCryption.
pub fn is_cbc(text: &str) -> bool {
    text.starts_with("+OK *") || text.starts_with("mcps *")
}

fn ecb_payload(text: &str) -> Option<&str> {
    let payload = text
        .strip_prefix(PREFIX)
        .or_else(|| text.strip_prefix(MCPS_PREFIX))?;
    (!payload.starts_with('*')).then_some(payload)
}

fn valid_payload(payload: &str) -> bool {
    !payload.is_empty()
        && payload.len() % 12 == 0
        && payload.bytes().all(|byte| FISH_B64.contains(&byte))
}

pub fn encrypt_ecb(key: &[u8], text: &str) -> Result<String, FishError> {
    if !(MIN_KEY..=MAX_KEY).contains(&key.len()) {
        return Err(FishError::InvalidKey);
    }
    let cipher = Blowfish::new_from_slice(key).map_err(|_| FishError::InvalidKey)?;
    let mut data = text.as_bytes().to_vec();
    let remainder = data.len() % 8;
    if remainder != 0 {
        data.resize(data.len() + (8 - remainder), 0);
    }
    for block in data.chunks_exact_mut(8) {
        crypt_block(&cipher, block, true);
    }
    Ok(format!("{PREFIX}{}", fish_b64_encode(&data)))
}

pub fn decrypt_ecb(key: &[u8], text: &str) -> Result<String, FishError> {
    if is_cbc(text) {
        return Err(FishError::UnsupportedMode);
    }
    let payload = ecb_payload(text).ok_or(FishError::InvalidCiphertext)?;
    if !valid_payload(payload) {
        return Err(FishError::InvalidCiphertext);
    }
    if !(MIN_KEY..=MAX_KEY).contains(&key.len()) {
        return Err(FishError::InvalidKey);
    }
    let mut data = fish_b64_decode(payload)?;
    let cipher = Blowfish::new_from_slice(key).map_err(|_| FishError::InvalidKey)?;
    for block in data.chunks_exact_mut(8) {
        crypt_block(&cipher, block, false);
    }
    while data.last() == Some(&0) {
        data.pop();
    }
    String::from_utf8(data).map_err(|_| FishError::InvalidUtf8)
}

fn crypt_block(cipher: &Blowfish, block: &mut [u8], encrypt: bool) {
    let mut raw = Block::<Blowfish>::default();
    raw.copy_from_slice(block);
    if encrypt {
        cipher.encrypt_block(&mut raw);
    } else {
        cipher.decrypt_block(&mut raw);
    }
    block.copy_from_slice(&raw);
}

fn fish_b64_encode(data: &[u8]) -> String {
    debug_assert_eq!(data.len() % 8, 0);
    let mut out = String::with_capacity(data.len() / 8 * 12);
    for block in data.chunks_exact(8) {
        let left = u32::from_be_bytes(block[..4].try_into().expect("bloc de 4 octets"));
        let right = u32::from_be_bytes(block[4..].try_into().expect("bloc de 4 octets"));
        // FiSH écrit d'abord le mot de droite, poids faible en tête.
        for mut value in [right, left] {
            for _ in 0..6 {
                out.push(FISH_B64[(value & 0x3f) as usize] as char);
                value >>= 6;
            }
        }
    }
    out
}

fn fish_b64_decode(payload: &str) -> Result<Vec<u8>, FishError> {
    let bytes = payload.as_bytes();
    if bytes.is_empty() || bytes.len() % 12 != 0 {
        return Err(FishError::InvalidCiphertext);
    }
    let mut out = Vec::with_capacity(bytes.len() / 12 * 8);
    for chunk in bytes.chunks_exact(12) {
        let mut right = 0u32;
        for (index, byte) in chunk[..6].iter().enumerate() {
            right |= u32::from(fish_b64_value(*byte)?) << (6 * index);
        }
        let mut left = 0u32;
        for (index, byte) in chunk[6..].iter().enumerate() {
            left |= u32::from(fish_b64_value(*byte)?) << (6 * index);
        }
        out.extend_from_slice(&left.to_be_bytes());
        out.extend_from_slice(&right.to_be_bytes());
    }
    Ok(out)
}

fn fish_b64_value(byte: u8) -> Result<u8, FishError> {
    FISH_B64
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|index| index as u8)
        .ok_or(FishError::InvalidCiphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("le-salon-{name}-{stamp}.json"))
    }

    #[test]
    fn golden_ecb_vectors() {
        let key = b"blowfish";
        let cases = [
            ("qwertyui", "+OK qKKBR.riU410"),
            ("qwerty", "+OK Ee5Cz.Dgr1q1"),
            ("qwertyuiop", "+OK qKKBR.riU410bpm4f/1UAmL0"),
            (
                "qwertyuiopasdfghjklzxcvbnm1234567890",
                "+OK qKKBR.riU410sK.30/GVDts.ewTqf.5tm3p/Ol.d.0DH23r/m9tTw.Rmpbj1",
            ),
        ];
        for (plain, ciphertext) in cases {
            assert_eq!(encrypt_ecb(key, plain).unwrap(), ciphertext);
            assert_eq!(decrypt_ecb(key, ciphertext).unwrap(), plain);
        }
        assert_eq!(decrypt_ecb(key, "mcps qKKBR.riU410").unwrap(), "qwertyui");
        assert_eq!(
            decrypt_ecb(key, &encrypt_ecb(key, "héllo").unwrap()).unwrap(),
            "héllo"
        );
    }

    #[test]
    fn detects_and_reports_cbc() {
        assert!(is_encrypted("+OK *o2BNTv01lAIXRSFP+Efw/w=="));
        assert_eq!(
            decrypt_ecb(b"blowfish", "+OK *o2BNTv01lAIXRSFP+Efw/w=="),
            Err(FishError::UnsupportedMode)
        );
    }

    #[test]
    fn rejects_invalid_keys_and_payloads() {
        assert_eq!(encrypt_ecb(b"abc", "test"), Err(FishError::InvalidKey));
        assert_eq!(
            decrypt_ecb(b"blowfish", "+OK pas-du-fiSH"),
            Err(FishError::InvalidCiphertext)
        );
        assert!(!is_encrypted("+OK bonjour"));
        assert!(!is_encrypted("bonjour"));
    }

    #[test]
    fn store_roundtrip() {
        let path = temp_path("fish");
        let mut store = FishStore::new(&path);
        store
            .set_text("irc.test:6667", "#salon", "clé secrète")
            .unwrap();
        assert!(store.has_key("IRC.TEST:6667", "#SALON"));

        let reloaded = FishStore::new(&path);
        assert_eq!(
            reloaded.key_for("irc.test:6667", "#salon").unwrap(),
            "clé secrète".as_bytes()
        );
        assert_eq!(reloaded.targets("irc.test:6667"), vec!["#salon"]);

        let mut store = reloaded;
        assert!(store.remove("irc.test:6667", "#salon"));
        assert!(!FishStore::new(&path).has_key("irc.test:6667", "#salon"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn keys_are_scoped_per_network() {
        let path = temp_path("fish-networks");
        let mut store = FishStore::new(&path);
        store.set_text("a.test:6667", "#salon", "clé-a").unwrap();
        store.set_text("b.test:6667", "#salon", "clé-b").unwrap();
        assert_eq!(
            store.key_for("a.test:6667", "#salon").unwrap(),
            "clé-a".as_bytes()
        );
        assert_eq!(
            store.key_for("b.test:6667", "#salon").unwrap(),
            "clé-b".as_bytes()
        );
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn store_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let path = temp_path("fish-permissions");
        let mut store = FishStore::new(&path);
        store.set_text("irc.test:6667", "#salon", "secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn custom_base64_roundtrip() {
        let data = b"12345678abcdefgh";
        assert_eq!(fish_b64_decode(&fish_b64_encode(data)).unwrap(), data);
    }
}
