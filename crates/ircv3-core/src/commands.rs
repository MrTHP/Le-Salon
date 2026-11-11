//! Analyse des commandes saisies par l'utilisateur.
//!
//! Le parseur est pur (aucun accès réseau) ; le protocole transforme ensuite
//! `ParsedCommand` en lignes IRC et en actions locales.

use crate::{message::is_channel_name, model::BufferKind};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBuffer {
    pub kind: BufferKind,
    pub name: String,
}

impl ActiveBuffer {
    pub fn target(&self) -> Option<&str> {
        match self.kind {
            BufferKind::Server => None,
            BufferKind::Channel | BufferKind::Query => Some(&self.name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalCommand {
    Clear,
    Close,
    Help(Option<String>),
    Version,
    Time,
    Quit(Option<String>),
    Reconnect,
    Disconnect,
    PartAll,
    Error(String),
    Info(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedCommand {
    Say {
        target: String,
        text: String,
    },
    Action {
        target: String,
        text: String,
    },
    Notice {
        target: String,
        text: String,
    },
    Ctcp {
        target: String,
        payload: String,
    },
    Join {
        channels: String,
        keys: Option<String>,
    },
    Part {
        channels: String,
        reason: Option<String>,
    },
    Cycle {
        channel: String,
    },
    Nick(String),
    Query {
        target: String,
        text: Option<String>,
    },
    Reply {
        target: String,
        msgid: String,
        text: String,
    },
    React {
        target: String,
        msgid: String,
        emoji: String,
        remove: bool,
    },
    Topic {
        channel: Option<String>,
        text: Option<String>,
    },
    Names(Option<String>),
    Whois(String),
    Who(Option<String>),
    Whowas(String),
    List(Option<String>),
    Mode {
        target: String,
        modes: String,
    },
    UserMode {
        channel: Option<String>,
        mode: char,
        add: bool,
        nicks: Vec<String>,
    },
    Kick {
        channel: Option<String>,
        nick: String,
        reason: Option<String>,
    },
    Ban {
        channel: Option<String>,
        mask: String,
        remove: bool,
    },
    Invite {
        nick: String,
        channel: Option<String>,
    },
    Away(Option<String>),
    Monitor(String),
    Raw(String),
    SetName(String),
    Ping(Option<String>),
    Oper {
        name: String,
        password: String,
    },
    Wallops(String),
    Local(LocalCommand),
}

pub fn parse(input: &str, active: Option<&ActiveBuffer>, _nick: &str) -> ParsedCommand {
    let input = input.trim_end();
    if input.is_empty() {
        return ParsedCommand::Local(LocalCommand::Info(String::new()));
    }
    if !input.starts_with('/') {
        return match active.and_then(ActiveBuffer::target) {
            Some(target) => ParsedCommand::Say {
                target: target.to_string(),
                text: input.to_string(),
            },
            None => ParsedCommand::Local(LocalCommand::Error(
                "Sélectionnez un salon ou un privé avant d'écrire.".into(),
            )),
        };
    }
    if let Some(literal) = input.strip_prefix("//") {
        return match active.and_then(ActiveBuffer::target) {
            Some(target) => ParsedCommand::Say {
                target: target.to_string(),
                text: format!("/{literal}"),
            },
            None => ParsedCommand::Local(LocalCommand::Error(
                "Aucun salon actif pour envoyer ce message.".into(),
            )),
        };
    }

    let without_slash = &input[1..];
    let (command, rest) = match without_slash.split_once(char::is_whitespace) {
        Some((command, rest)) => (command.to_ascii_lowercase(), rest.trim_start()),
        None => (without_slash.to_ascii_lowercase(), ""),
    };
    let target = active.and_then(ActiveBuffer::target).map(str::to_string);
    let (first, after_first) = split_first(rest);

    match command.as_str() {
        "j" | "join" => {
            if first.is_empty() {
                return local_error("/join #salon");
            }
            let channels = normalize_channels(first);
            ParsedCommand::Join {
                channels,
                keys: (!after_first.is_empty()).then(|| after_first.to_string()),
            }
        }
        "leave" | "part" => {
            let (channels, reason) = if is_channel_name(first) {
                (first.to_string(), non_empty(after_first))
            } else if let Some(target) = target {
                (target, non_empty(rest))
            } else {
                return local_error("/part : aucun salon actif");
            };
            ParsedCommand::Part { channels, reason }
        }
        "cycle" => {
            let channel = if is_channel_name(first) {
                first.to_string()
            } else if let Some(target) = target {
                target
            } else {
                return local_error("/cycle : aucun salon actif");
            };
            ParsedCommand::Cycle { channel }
        }
        "nick" => {
            if first.is_empty() {
                local_error("/nick nouveau_pseudo")
            } else {
                ParsedCommand::Nick(first.to_string())
            }
        }
        "msg" | "query" => {
            if first.is_empty() {
                return local_error("/msg pseudo texte");
            }
            let text = non_empty(strip_colon(after_first));
            ParsedCommand::Query {
                target: first.to_string(),
                text,
            }
        }
        "reply" => {
            let Some(target) = target else {
                return local_error("/reply : aucun salon actif");
            };
            if first.is_empty() || after_first.is_empty() {
                return local_error("/reply identifiant texte");
            }
            ParsedCommand::Reply {
                target,
                msgid: first.to_string(),
                text: strip_colon(after_first).to_string(),
            }
        }
        "react" | "unreact" => {
            let Some(target) = target else {
                return local_error("/react : aucun salon actif");
            };
            if first.is_empty() || after_first.is_empty() {
                return local_error("/react identifiant emoji");
            }
            ParsedCommand::React {
                target,
                msgid: first.to_string(),
                emoji: after_first.trim().to_string(),
                remove: command == "unreact",
            }
        }
        "me" | "action" => {
            if let Some(text) = non_empty(rest) {
                let (target, text) = if is_channel_name(first) || first.contains('!') {
                    (first.to_string(), strip_colon(after_first).to_string())
                } else {
                    match target {
                        Some(target) => (target, text),
                        None => return local_error("/me : aucun salon actif"),
                    }
                };
                ParsedCommand::Action { target, text }
            } else {
                local_error("/me action")
            }
        }
        "say" => match target {
            Some(target) => ParsedCommand::Say {
                target,
                text: rest.to_string(),
            },
            None => local_error("/say : aucun salon actif"),
        },
        "notice" => {
            if first.is_empty() {
                return local_error("/notice cible texte");
            }
            let text = strip_colon(after_first);
            if text.is_empty() {
                local_error("/notice cible texte")
            } else {
                ParsedCommand::Notice {
                    target: first.to_string(),
                    text: text.to_string(),
                }
            }
        }
        "ctcp" => {
            if first.is_empty() || after_first.is_empty() {
                return local_error("/ctcp cible requête");
            } else {
                ParsedCommand::Ctcp {
                    target: first.to_string(),
                    payload: after_first.to_string(),
                }
            }
        }
        "topic" => {
            if first.is_empty() {
                ParsedCommand::Topic {
                    channel: target,
                    text: None,
                }
            } else if is_channel_name(first) {
                ParsedCommand::Topic {
                    channel: Some(first.to_string()),
                    text: non_empty(strip_colon(after_first)),
                }
            } else {
                ParsedCommand::Topic {
                    channel: target,
                    text: Some(strip_colon(rest).to_string()),
                }
            }
        }
        "names" => ParsedCommand::Names(if is_channel_name(first) {
            Some(first.to_string())
        } else {
            target
        }),
        "whois" => {
            if first.is_empty() {
                local_error("/whois pseudo")
            } else {
                ParsedCommand::Whois(rest.to_string())
            }
        }
        "who" => ParsedCommand::Who(non_empty(rest)),
        "whowas" => {
            if first.is_empty() {
                local_error("/whowas pseudo")
            } else {
                ParsedCommand::Whowas(rest.to_string())
            }
        }
        "list" => ParsedCommand::List(non_empty(rest)),
        "mode" => {
            if first.is_empty() {
                match target {
                    Some(target) => ParsedCommand::Mode {
                        target,
                        modes: String::new(),
                    },
                    None => local_error("/mode cible modes"),
                }
            } else if (first.starts_with('+') || first.starts_with('-')) && target.is_some() {
                ParsedCommand::Mode {
                    target: target.unwrap(),
                    modes: rest.to_string(),
                }
            } else {
                ParsedCommand::Mode {
                    target: first.to_string(),
                    modes: after_first.to_string(),
                }
            }
        }
        "op" | "deop" | "voice" | "devoice" | "halfop" | "dehalfop" => {
            let mode = match command.as_str() {
                "op" => '+',
                "deop" => '-',
                "voice" => '+',
                "devoice" => '-',
                "halfop" => '+',
                "dehalfop" => '-',
                _ => '+',
            };
            let mode_char = match command.as_str() {
                "voice" | "devoice" => 'v',
                "halfop" | "dehalfop" => 'h',
                _ => 'o',
            };
            parse_user_mode(first, rest, target, mode, mode_char)
        }
        "kick" => {
            let (channel, nick, reason) = if is_channel_name(first) {
                let (nick, reason) = split_first(after_first);
                if nick.is_empty() {
                    return local_error("/kick [#salon] pseudo [raison]");
                }
                (Some(first.to_string()), nick.to_string(), non_empty(reason))
            } else if let Some(target) = target {
                if first.is_empty() {
                    return local_error("/kick pseudo [raison]");
                }
                (Some(target), first.to_string(), non_empty(after_first))
            } else {
                return local_error("/kick [#salon] pseudo [raison]");
            };
            ParsedCommand::Kick {
                channel,
                nick,
                reason,
            }
        }
        "ban" | "unban" => {
            let remove = command == "unban";
            let (channel, mask) = if is_channel_name(first) {
                (Some(first.to_string()), after_first.to_string())
            } else {
                (target, rest.to_string())
            };
            if mask.is_empty() {
                local_error("/ban [#salon] masque")
            } else {
                ParsedCommand::Ban {
                    channel,
                    mask,
                    remove,
                }
            }
        }
        "invite" => {
            if first.is_empty() {
                return local_error("/invite pseudo [#salon]");
            }
            let channel = if is_channel_name(after_first) {
                Some(after_first.to_string())
            } else {
                target
            };
            ParsedCommand::Invite {
                nick: first.to_string(),
                channel,
            }
        }
        "away" => ParsedCommand::Away(non_empty(rest)),
        "back" => ParsedCommand::Away(None),
        "monitor" => ParsedCommand::Monitor(if rest.is_empty() {
            "L".to_string()
        } else {
            rest.to_string()
        }),
        "raw" | "quote" => {
            if rest.is_empty() {
                local_error("/raw ligne IRC")
            } else {
                ParsedCommand::Raw(rest.to_string())
            }
        }
        "setname" => {
            if rest.is_empty() {
                local_error("/setname nom réel")
            } else {
                ParsedCommand::SetName(rest.to_string())
            }
        }
        "ping" => ParsedCommand::Ping(non_empty(rest)),
        "oper" => {
            let (name, password) = split_first(rest);
            if name.is_empty() || password.is_empty() {
                local_error("/oper nom motdepasse")
            } else {
                ParsedCommand::Oper {
                    name: name.to_string(),
                    password: password.to_string(),
                }
            }
        }
        "wallops" => ParsedCommand::Wallops(rest.to_string()),
        "clear" => ParsedCommand::Local(LocalCommand::Clear),
        "close" => ParsedCommand::Local(LocalCommand::Close),
        "help" | "aide" => ParsedCommand::Local(LocalCommand::Help(non_empty(rest))),
        "version" => ParsedCommand::Local(LocalCommand::Version),
        "time" | "heure" => ParsedCommand::Local(LocalCommand::Time),
        "quit" | "exit" => ParsedCommand::Local(LocalCommand::Quit(non_empty(rest))),
        "reconnect" => ParsedCommand::Local(LocalCommand::Reconnect),
        "disconnect" => ParsedCommand::Local(LocalCommand::Disconnect),
        "partall" => ParsedCommand::Local(LocalCommand::PartAll),
        "dcc" | "jitsi" | "upload" | "file" | "schedule" | "planifie" | "emoji" | "video" => {
            ParsedCommand::Local(LocalCommand::Error(
                "Cette fonction du client web n'est pas disponible dans le client GPUI.".into(),
            ))
        }
        "server" => ParsedCommand::Local(LocalCommand::Error(
            "Utilisez le bouton « Nouveau réseau » pour changer de serveur.".into(),
        )),
        _ => local_error(&format!("/{command} : commande inconnue (essayez /aide)")),
    }
}

fn parse_user_mode(
    first: &str,
    rest: &str,
    target: Option<String>,
    sign: char,
    mode: char,
) -> ParsedCommand {
    let (channel, nicks_text) = if is_channel_name(first) {
        (Some(first.to_string()), rest)
    } else {
        (target, rest)
    };
    let nicks: Vec<String> = nicks_text.split_whitespace().map(str::to_string).collect();
    if nicks.is_empty() {
        return local_error(&format!("/{mode} pseudo… : aucun pseudo"));
    }
    ParsedCommand::UserMode {
        channel,
        mode,
        add: sign == '+',
        nicks,
    }
}

fn local_error(message: &str) -> ParsedCommand {
    ParsedCommand::Local(LocalCommand::Error(message.to_string()))
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn strip_colon(value: &str) -> &str {
    value.strip_prefix(':').unwrap_or(value)
}

fn split_first(value: &str) -> (&str, &str) {
    match value.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest.trim_start()),
        None => (value, ""),
    }
}

fn normalize_channels(value: &str) -> String {
    value
        .split(',')
        .filter(|part| !part.is_empty())
        .map(|part| {
            if part.starts_with(['#', '&', '+', '!']) {
                part.to_string()
            } else {
                format!("#{part}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

pub fn help_lines() -> Vec<&'static str> {
    if crate::i18n::lang() == crate::i18n::Lang::En {
        vec![
            "Available commands:",
            "  /join #channel [key]     /part [#channel] [reason]   /cycle [#channel]",
            "  /nick nickname           /msg target text           /query target [text]",
            "  /me action               /notice target text        /ctcp target request",
            "  /topic [#channel] [text] /names [#channel]          /mode target modes",
            "  /op|/deop nick…          /voice|/devoice nick…      /kick nick [reason]",
            "  /ban|/unban mask         /invite nick [#channel]    /away [message]",
            "  /monitor +|- nick        /whois nick                /who [mask]",
            "  /list [filter]           /setname real name         /ping [target]",
            "  /raw IRC line            /clear                     /close",
            "  /reply msgid text        /react msgid emoji         /unreact msgid emoji",
            "  /reconnect               /disconnect                /quit [reason]",
            "  /emoji                   /jitsi [room]              /upload [file]",
            "  /paste                   /share [msgid]             /excerpt [n]",
            "  /quiet 22:00 08:00       /quiet off                 /sound on|off",
            "  /mute                    /proxy host:port           /proxy off",
            "  /encrypt generate|set|set-key|list|remove",
            "  /setkey [target] key     /delkey [target]           /showkey [target]",
            "  /fish [list|status]      FiSH = Blowfish ECB compatibility only",
            "  /schedule +10m text      /dcc send nick file        /dcc cancel",
            "",
            "Shortcuts: Tab completes a nickname, ↑/↓ browses history,",
            "Alt+1..9 switches tabs, Ctrl+L clears the active buffer.",
        ]
    } else {
        vec![
            "Commandes disponibles :",
            "  /join #salon [clé]      /part [#salon] [raison]    /cycle [#salon]",
            "  /nick pseudo             /msg cible texte          /query cible [texte]",
            "  /me action               /notice cible texte       /ctcp cible requête",
            "  /topic [#salon] [sujet]  /names [#salon]           /mode cible modes",
            "  /op|/deop pseudo…        /voice|/devoice pseudo…   /kick pseudo [raison]",
            "  /ban|/unban masque       /invite pseudo [#salon]   /away [message]",
            "  /monitor +|- pseudo      /whois pseudo             /who [masque]",
            "  /list [filtre]           /setname nom réel         /ping [cible]",
            "  /raw ligne IRC           /clear                    /close",
            "  /reply msgid texte       /react msgid emoji        /unreact msgid emoji",
            "  /reconnect               /disconnect               /quit [raison]",
            "  /emoji                   /jitsi [salle]            /upload [fichier]",
            "  /paste                   /share [msgid]             /excerpt [n]",
            "  /quiet 22:00 08:00       /quiet off                 /sound on|off",
            "  /mute                    /proxy hôte:port           /proxy off",
            "  /encrypt generate|set|set-key|list|remove",
            "  /setkey [cible] clé      /delkey [cible]            /showkey [cible]",
            "  /fish [list|status]      FiSH = Blowfish ECB, compatibilité",
            "  /schedule +10m texte     /dcc send pseudo fichier  /dcc cancel",
            "",
            "Raccourcis : Tab complète un pseudo, ↑/↓ parcourt l'historique,",
            "Alt+1..9 change d'onglet, Ctrl+L efface le tampon actif.",
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(name: &str) -> ActiveBuffer {
        ActiveBuffer {
            kind: BufferKind::Channel,
            name: name.into(),
        }
    }

    #[test]
    fn plain_message_goes_to_active() {
        let active = channel("#test");
        assert_eq!(
            parse("bonjour", Some(&active), "moi"),
            ParsedCommand::Say {
                target: "#test".into(),
                text: "bonjour".into()
            }
        );
    }

    #[test]
    fn literal_slash() {
        let active = channel("#test");
        assert_eq!(
            parse("//join pas une commande", Some(&active), "moi"),
            ParsedCommand::Say {
                target: "#test".into(),
                text: "/join pas une commande".into()
            }
        );
    }

    #[test]
    fn join_normalizes() {
        assert_eq!(
            parse("/join test,#autre clé", Some(&channel("#x")), "moi"),
            ParsedCommand::Join {
                channels: "#test,#autre".into(),
                keys: Some("clé".into())
            }
        );
    }

    #[test]
    fn part_uses_active_or_first_channel() {
        assert_eq!(
            parse("/part", Some(&channel("#x")), "moi"),
            ParsedCommand::Part {
                channels: "#x".into(),
                reason: None
            }
        );
        assert_eq!(
            parse("/part #y à plus", Some(&channel("#x")), "moi"),
            ParsedCommand::Part {
                channels: "#y".into(),
                reason: Some("à plus".into())
            }
        );
    }

    #[test]
    fn me_action() {
        assert_eq!(
            parse("/me fait un essai", Some(&channel("#x")), "moi"),
            ParsedCommand::Action {
                target: "#x".into(),
                text: "fait un essai".into()
            }
        );
    }

    #[test]
    fn msg_and_query() {
        assert_eq!(
            parse("/msg alice :salut", Some(&channel("#x")), "moi"),
            ParsedCommand::Query {
                target: "alice".into(),
                text: Some("salut".into())
            }
        );
        assert_eq!(
            parse("/query alice", Some(&channel("#x")), "moi"),
            ParsedCommand::Query {
                target: "alice".into(),
                text: None
            }
        );
    }

    #[test]
    fn reply_and_react_commands() {
        let active = channel("#test");
        assert_eq!(
            parse("/reply msg-1 bonjour", Some(&active), "moi"),
            ParsedCommand::Reply {
                target: "#test".into(),
                msgid: "msg-1".into(),
                text: "bonjour".into()
            }
        );
        assert_eq!(
            parse("/react msg-1 👍", Some(&active), "moi"),
            ParsedCommand::React {
                target: "#test".into(),
                msgid: "msg-1".into(),
                emoji: "👍".into(),
                remove: false
            }
        );
        assert_eq!(
            parse("/unreact msg-1 👍", Some(&active), "moi"),
            ParsedCommand::React {
                target: "#test".into(),
                msgid: "msg-1".into(),
                emoji: "👍".into(),
                remove: true
            }
        );
    }

    #[test]
    fn topic_variants() {
        assert_eq!(
            parse("/topic", Some(&channel("#x")), "moi"),
            ParsedCommand::Topic {
                channel: Some("#x".into()),
                text: None
            }
        );
        assert_eq!(
            parse("/topic #y nouveau sujet", Some(&channel("#x")), "moi"),
            ParsedCommand::Topic {
                channel: Some("#y".into()),
                text: Some("nouveau sujet".into())
            }
        );
        assert_eq!(
            parse("/topic sujet ici", Some(&channel("#x")), "moi"),
            ParsedCommand::Topic {
                channel: Some("#x".into()),
                text: Some("sujet ici".into())
            }
        );
    }

    #[test]
    fn op_and_ban() {
        assert_eq!(
            parse("/op alice bob", Some(&channel("#x")), "moi"),
            ParsedCommand::UserMode {
                channel: Some("#x".into()),
                mode: 'o',
                add: true,
                nicks: vec!["alice".into(), "bob".into()]
            }
        );
        assert_eq!(
            parse("/ban #y *!*@spam", Some(&channel("#x")), "moi"),
            ParsedCommand::Ban {
                channel: Some("#y".into()),
                mask: "*!*@spam".into(),
                remove: false
            }
        );
        assert_eq!(
            parse("/unban *!*@spam", Some(&channel("#x")), "moi"),
            ParsedCommand::Ban {
                channel: Some("#x".into()),
                mask: "*!*@spam".into(),
                remove: true
            }
        );
    }

    #[test]
    fn kick_variants() {
        assert_eq!(
            parse("/kick alice raison", Some(&channel("#x")), "moi"),
            ParsedCommand::Kick {
                channel: Some("#x".into()),
                nick: "alice".into(),
                reason: Some("raison".into())
            }
        );
        assert_eq!(
            parse("/kick #y alice", Some(&channel("#x")), "moi"),
            ParsedCommand::Kick {
                channel: Some("#y".into()),
                nick: "alice".into(),
                reason: None
            }
        );
    }

    #[test]
    fn local_commands() {
        assert_eq!(
            parse("/clear", Some(&channel("#x")), "moi"),
            ParsedCommand::Local(LocalCommand::Clear)
        );
        assert_eq!(
            parse("/quit à demain", Some(&channel("#x")), "moi"),
            ParsedCommand::Local(LocalCommand::Quit(Some("à demain".into())))
        );
        assert!(matches!(
            parse("/dcc send alice", Some(&channel("#x")), "moi"),
            ParsedCommand::Local(LocalCommand::Error(_))
        ));
    }

    #[test]
    fn unknown_command_is_local_error() {
        assert!(matches!(
            parse("/nawak", Some(&channel("#x")), "moi"),
            ParsedCommand::Local(LocalCommand::Error(_))
        ));
    }
}
