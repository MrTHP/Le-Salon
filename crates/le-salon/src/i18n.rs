//! Traductions de l'interface. La langue globale est partagée avec
//! `ircv3-core` pour que les messages locaux du protocole suivent aussi.

pub use ircv3_core::i18n::{Lang, lang, set_lang, t};

pub fn is_fr() -> bool {
    lang() == Lang::Fr
}
