//! ircv3-core — socle partagé du client IRCv3.
//!
//! Contient :
//!   * l'analyseur de messages IRC / IRCv3 (tags, préfixe, paramètres) ;
//!   * le formatage mIRC (couleurs, gras, italique, souligné…) ;
//!   * le modèle de session (réseaux, salons, privés, pseudos, historiques) ;
//!   * l'exécution des commandes utilisateur (/join, /msg, /mode…) ;
//!   * le transport TCP / TLS (rustls) et la boucle réseau.
//!
//! Le crate ne dépend d'aucune interface graphique : il est utilisable par
//! GPUI, par les tests, ou par un éventuel client console.

pub mod client;
pub mod commands;
pub mod config;
pub mod dcc;
pub mod formatting;
pub mod i18n;
pub mod media;
pub mod message;
pub mod model;
pub mod protocol;
pub mod secrets;
pub mod upload;

pub use config::{Config, ConnectionConfig};
pub use i18n::Lang;
pub use message::{Message, Prefix, Tag};
pub use model::{
    AppModel, Buffer, BufferId, BufferKind, ChatLine, ConnectionStatus, LineKind, Network,
    NetworkId, User,
};

/// Réveil de l'interface : le thread réseau envoie un jeton, la vue GPUI
/// redessine. Le canal est borné à 1 pour fusionner les notifications.
pub type Wake = async_channel::Sender<()>;

/// Version affichée dans l'interface et les réponses CTCP VERSION.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_NAME: &str = "Le Salon";

/// Abaisse une chaîne selon le casemapping RFC 1459 utilisé par IRC.
///
/// `[]\^` sont équivalents à `{}|~`, et les lettres ASCII sont insensibles
/// à la casse. Les caractères non ASCII sont conservés tels quels.
pub fn irc_fold(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' => c.to_ascii_lowercase(),
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            '^' => '~',
            _ => c,
        })
        .collect()
}

/// Vrai si le pseudo est mentionné comme mot entier dans le texte.
pub fn mentions(text: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let text = irc_fold(text);
    let nick = irc_fold(nick);
    let is_boundary = |c: char| {
        !(c.is_alphanumeric()
            || matches!(
                c,
                '[' | ']' | '\\' | '^' | '{' | '}' | '|' | '~' | '_' | '-'
            ))
    };
    for (idx, _) in text.char_indices() {
        if !text[idx..].starts_with(&nick) {
            continue;
        }
        let before_ok = text[..idx].chars().next_back().is_none_or(is_boundary);
        let after = idx + nick.len();
        let after_ok = text[after..].chars().next().is_none_or(is_boundary);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// Tronque une ligne IRC à la limite de 512 octets (CRLF compris) sans
/// couper un caractère UTF-8.
pub fn truncate_irc_line(line: &str) -> String {
    let line = line.replace(['\r', '\n'], " ");
    if line.len() <= 510 {
        return line;
    }
    let mut end = 510;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_rfc1459() {
        assert_eq!(irc_fold("Nick[]\\^"), "nick{}|~");
    }

    #[test]
    fn mention_is_word_bounded() {
        assert!(mentions("salut alice, ça va ?", "alice"));
        assert!(mentions("@alice: regarde", "alice"));
        assert!(!mentions("alice2 est là", "alice"));
        assert!(!mentions("malice", "alice"));
        assert!(mentions("ALICE", "alice"));
    }

    #[test]
    fn truncate_keeps_utf8() {
        let line = "é".repeat(400);
        let out = truncate_irc_line(&line);
        assert!(out.len() <= 510);
        assert!(out.is_char_boundary(out.len()));
    }
}
