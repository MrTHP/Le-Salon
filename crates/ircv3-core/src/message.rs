//! Analyseur de messages IRC et de tags IRCv3.
//!
//! Grammaire (RFC 1459 / IRCv3) :
//!   [@tags] [:prefix] COMMAND [params…] [:trailing]
//!
//! Les tags sont déséchappés selon la spécification IRCv3 :
//! `\:` -> `;`, `\s` -> espace, `\\` -> `\`, `\r`, `\n`.

use std::fmt;

/// Un tag IRCv3. `value == None` signifie « tag sans valeur » ; une valeur
/// vide est représentée par `Some("")`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub key: String,
    pub value: Option<String>,
}

impl Tag {
    pub fn new(key: impl Into<String>, value: Option<impl Into<String>>) -> Self {
        Self {
            key: key.into(),
            value: value.map(Into::into),
        }
    }
}

/// Préfixe d'un message : pseudo (ou serveur), utilisateur, hôte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prefix {
    pub nick: String,
    pub user: Option<String>,
    pub host: Option<String>,
}

impl Prefix {
    pub fn parse(raw: &str) -> Self {
        let (nick, rest) = match raw.split_once('!') {
            Some((nick, rest)) => (nick, Some(rest)),
            None => (raw, None),
        };
        let (user, host) = match rest {
            Some(rest) => match rest.split_once('@') {
                Some((user, host)) => (Some(user.to_string()), Some(host.to_string())),
                None => (Some(rest.to_string()), None),
            },
            None => match nick.split_once('@') {
                Some((user, host)) => (Some(user.to_string()), Some(host.to_string())),
                None => (None, None),
            },
        };
        let nick = match (rest, user.as_deref()) {
            (None, Some(_)) => raw.split('@').next().unwrap_or(raw).to_string(),
            _ => nick.to_string(),
        };
        Self { nick, user, host }
    }

    pub fn is_server(&self) -> bool {
        self.user.is_none() && self.host.is_none()
    }

    pub fn nick(&self) -> &str {
        &self.nick
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

/// Un message IRC analysé.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub tags: Vec<Tag>,
    pub prefix: Option<Prefix>,
    /// Commande en majuscules (`PRIVMSG`, `001`, `CAP`…).
    pub command: String,
    pub params: Vec<String>,
}

impl Message {
    pub fn parse(line: &str) -> Result<Self, ParseError> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Err(ParseError("ligne vide".into()));
        }

        let mut rest = line;
        let mut tags = Vec::new();
        if let Some(after_at) = rest.strip_prefix('@') {
            let (tag_part, after) = after_at
                .split_once(' ')
                .ok_or_else(|| ParseError("tags sans commande".into()))?;
            tags = parse_tags(tag_part)?;
            rest = after.trim_start();
        }

        let mut prefix = None;
        if let Some(after_colon) = rest.strip_prefix(':') {
            let (prefix_part, after) = after_colon
                .split_once(' ')
                .ok_or_else(|| ParseError("préfixe sans commande".into()))?;
            prefix = Some(Prefix::parse(prefix_part));
            rest = after.trim_start();
        }

        if rest.is_empty() {
            return Err(ParseError("commande absente".into()));
        }

        let (command, after) = match rest.split_once(' ') {
            Some((command, after)) => (command, after),
            None => (rest, ""),
        };
        if command.is_empty()
            || !command
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(ParseError(format!("commande invalide : {command:?}")));
        }

        Ok(Self {
            tags,
            prefix,
            command: command.to_ascii_uppercase(),
            params: parse_params(after),
        })
    }

    /// Valeur d'un tag (None si absent *ou* sans valeur).
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|tag| tag.key == key)
            .and_then(|tag| tag.value.as_deref())
    }

    /// Présence d'un tag, même sans valeur.
    pub fn has_tag(&self, key: &str) -> bool {
        self.tags.iter().any(|tag| tag.key == key)
    }

    pub fn nick(&self) -> Option<&str> {
        self.prefix.as_ref().map(|prefix| prefix.nick.as_str())
    }

    pub fn param(&self, index: usize) -> Option<&str> {
        self.params.get(index).map(String::as_str)
    }

    pub fn target(&self) -> Option<&str> {
        self.param(0)
    }

    /// Texte après le dernier paramètre (les paramètres de fin sont déjà
    /// fusionnés par l'analyseur).
    pub fn trailing(&self) -> Option<&str> {
        self.params.last().map(String::as_str)
    }

    /// Horodatage `server-time` (RFC 3339), converti en heure locale.
    pub fn server_time(&self) -> Option<chrono::DateTime<chrono::Local>> {
        use chrono::{DateTime, Local};
        self.tag("time")
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Local))
    }

    /// Vrai si la cible est un salon (selon les caractères usuels).
    pub fn target_is_channel(&self) -> bool {
        self.target().is_some_and(is_channel_name)
    }

    /// Reconstruit une ligne (utile aux tests et aux réponses brutes).
    pub fn to_line(&self) -> String {
        let mut out = String::new();
        if !self.tags.is_empty() {
            out.push('@');
            for (index, tag) in self.tags.iter().enumerate() {
                if index > 0 {
                    out.push(';');
                }
                out.push_str(&tag.key);
                if let Some(value) = &tag.value {
                    out.push('=');
                    out.push_str(&escape_tag_value(value));
                }
            }
            out.push(' ');
        }
        if let Some(prefix) = &self.prefix {
            out.push(':');
            out.push_str(&prefix.nick);
            if let Some(user) = &prefix.user {
                out.push('!');
                out.push_str(user);
            }
            if let Some(host) = &prefix.host {
                out.push('@');
                out.push_str(host);
            }
            out.push(' ');
        }
        out.push_str(&self.command);
        for (index, param) in self.params.iter().enumerate() {
            out.push(' ');
            let last = index + 1 == self.params.len();
            if last && (param.is_empty() || param.contains(' ')) {
                out.push(':');
            }
            out.push_str(param);
        }
        out
    }
}

pub fn is_channel_name(value: &str) -> bool {
    value.starts_with(['#', '&', '+', '!'])
}

fn parse_params(input: &str) -> Vec<String> {
    let mut params = Vec::new();
    let mut rest = input.trim_start();
    while !rest.is_empty() {
        if let Some(trailing) = rest.strip_prefix(':') {
            params.push(trailing.to_string());
            break;
        }
        match rest.split_once(' ') {
            Some((param, after)) => {
                if !param.is_empty() {
                    params.push(param.to_string());
                }
                rest = after.trim_start();
            }
            None => {
                params.push(rest.to_string());
                break;
            }
        }
    }
    params
}

fn parse_tags(input: &str) -> Result<Vec<Tag>, ParseError> {
    let mut tags = Vec::new();
    for raw in input.split(';') {
        if raw.is_empty() {
            continue;
        }
        let (key, value) = match raw.split_once('=') {
            Some((key, value)) => (key, Some(unescape_tag_value(value))),
            None => (raw, None),
        };
        if key.is_empty() {
            continue;
        }
        tags.push(Tag {
            key: key.to_string(),
            value,
        });
    }
    Ok(tags)
}

pub fn unescape_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(':') => out.push(';'),
            Some('s') => out.push(' '),
            Some('\\') => out.push('\\'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

pub fn escape_tag_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            ';' => out.push_str("\\:"),
            ' ' => out.push_str("\\s"),
            '\\' => out.push_str("\\\\"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_privmsg() {
        let msg = Message::parse(":alice!a@h PRIVMSG #test :bonjour tout le monde").unwrap();
        assert_eq!(msg.command, "PRIVMSG");
        assert_eq!(msg.nick(), Some("alice"));
        assert_eq!(msg.prefix.unwrap().user.as_deref(), Some("a"));
        assert_eq!(msg.params, vec!["#test", "bonjour tout le monde"]);
    }

    #[test]
    fn parse_tags_and_unscape() {
        let msg = Message::parse(
            "@time=2026-10-18T02:39:00.000Z;account=alice;+typing=active;flag;label=hi\\sthere\\:ok :a!b@c PRIVMSG #x :hi\\sthere\\:ok",
        )
        .unwrap();
        assert_eq!(msg.tag("account"), Some("alice"));
        assert_eq!(msg.tag("+typing"), Some("active"));
        assert_eq!(msg.tag("label"), Some("hi there;ok"));
        assert!(msg.has_tag("flag"));
        assert_eq!(msg.params[1], "hi\\sthere\\:ok");
        assert_eq!(msg.server_time().is_some(), true);
    }

    #[test]
    fn parse_no_prefix() {
        let msg = Message::parse("PING :123").unwrap();
        assert_eq!(msg.command, "PING");
        assert_eq!(msg.params, vec!["123"]);
        assert!(msg.prefix.is_none());
    }

    #[test]
    fn parse_server_prefix() {
        let msg = Message::parse(":irc.libera.chat 001 nick :Welcome").unwrap();
        assert_eq!(msg.command, "001");
        assert!(msg.prefix.unwrap().is_server());
    }

    #[test]
    fn parse_ipv6_prefix() {
        let msg = Message::parse(":nick!user@2001:db8::1 PRIVMSG #x :hello").unwrap();
        assert_eq!(msg.prefix.unwrap().host.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn parse_empty_trailing() {
        let msg = Message::parse("NOTICE nick :").unwrap();
        assert_eq!(msg.params, vec!["nick", ""]);
    }

    #[test]
    fn parse_multiple_spaces() {
        let msg = Message::parse("CAP  LS   302").unwrap();
        assert_eq!(msg.params, vec!["LS", "302"]);
    }

    #[test]
    fn parse_invalid() {
        assert!(Message::parse("").is_err());
        assert!(Message::parse("@a=b").is_err());
        assert!(Message::parse(":prefix").is_err());
    }

    #[test]
    fn tags_roundtrip() {
        let msg = Message::parse("@a=hello\\sworld;b PRIVMSG #x :y").unwrap();
        assert_eq!(msg.tag("a"), Some("hello world"));
        assert_eq!(msg.tags[1].value, None);
        let line = msg.to_line();
        let again = Message::parse(&line).unwrap();
        assert_eq!(again.tags, msg.tags);
    }

    #[test]
    fn channel_detection() {
        assert!(is_channel_name("#x"));
        assert!(is_channel_name("&x"));
        assert!(!is_channel_name("alice"));
    }

    #[test]
    fn utf8_trailing() {
        let msg = Message::parse("PRIVMSG #x :éàçù î").unwrap();
        assert_eq!(msg.params[1], "éàçù î");
    }
}
