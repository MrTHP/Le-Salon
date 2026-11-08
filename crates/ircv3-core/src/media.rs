//! Détection des liens média dans les messages (vidéo, audio, image).
//!
//! GPUI n'intègre pas de moteur web : le client affiche un bouton « ▶ » qui
//! ouvre le lien dans le navigateur/lecteur du système. Les services connus
//! sont reconnus par leur **hôte exact** (ou un sous-domaine), jamais par une
//! simple sous-chaîne : `https://evil.example/youtube.com/watch` n'est pas
//! une vidéo YouTube.

use crate::formatting::is_bidi_control;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    YouTube,
    Vimeo,
    Dailymotion,
    Streamable,
    Video,
    Audio,
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaLink {
    pub url: String,
    pub kind: MediaKind,
    /// Hôte affiché à côté du bouton pour que l'utilisateur sache où il va.
    pub host: String,
}

impl MediaKind {
    pub fn icon(self) -> &'static str {
        match self {
            Self::Audio => "♪",
            Self::Image => "▣",
            _ => "▶",
        }
    }

    pub fn is_video(self) -> bool {
        !matches!(self, Self::Audio | Self::Image)
    }
}

pub fn find_media(text: &str) -> Option<MediaLink> {
    for token in text.split_whitespace() {
        let url = token.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | ','
                    | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '«'
                    | '»'
                    | '"'
                    | '\''
            )
        });
        let Some(host) = host_of(url) else {
            continue;
        };
        let Some(kind) = classify(url, &host) else {
            continue;
        };
        return Some(MediaLink {
            url: url.to_string(),
            kind,
            host,
        });
    }
    None
}

/// Extrait l'hôte d'une URL HTTP(S), en ignorant l'éventuel userinfo
/// (`https://youtube.com@evil.example/…` a pour hôte `evil.example`).
fn host_of(url: &str) -> Option<String> {
    // Un antislash peut être interprété comme un séparateur par certains
    // navigateurs (`https://evil.com\@youtube.com/watch`). On refuse ces URL
    // ambiguës, ainsi que les caractères de contrôle.
    if url
        .chars()
        .any(|c| c == '\\' || c.is_control() || is_bidi_control(c))
    {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    let host = if let Some(after_bracket) = host_port.strip_prefix('[') {
        // Littéral IPv6 : on garde les crochets et on ignore le port.
        let end = after_bracket.find(']')?;
        &host_port[..end + 2]
    } else {
        host_port.split(':').next()?
    };
    let host = host.trim_end_matches('.');
    if host.is_empty() || host.starts_with('.') {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn is_host(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn classify(url: &str, host: &str) -> Option<MediaKind> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let after_authority = rest
        .find(['/', '?', '#'])
        .map(|index| &rest[index..])
        .unwrap_or("");
    let path = after_authority
        .split(['?', '#'])
        .next()
        .unwrap_or(after_authority);

    if is_host(host, "youtube.com")
        && (path.starts_with("/watch")
            || path.starts_with("/shorts/")
            || path.starts_with("/live/")
            || path.starts_with("/embed/"))
    {
        return Some(MediaKind::YouTube);
    }
    if host == "youtu.be" && path.len() > 1 {
        return Some(MediaKind::YouTube);
    }
    if is_host(host, "vimeo.com") && path.len() > 1 {
        return Some(MediaKind::Vimeo);
    }
    if (is_host(host, "dailymotion.com") && path.starts_with("/video/"))
        || (host == "dai.ly" && path.len() > 1)
    {
        return Some(MediaKind::Dailymotion);
    }
    if is_host(host, "streamable.com") && path.len() > 1 {
        return Some(MediaKind::Streamable);
    }

    let filename = path.rsplit('/').next().unwrap_or(path);
    let extension = filename.rsplit_once('.').map(|(_, extension)| extension)?;
    match extension.to_ascii_lowercase().as_str() {
        "mp4" | "webm" | "m3u8" | "mov" | "mkv" => Some(MediaKind::Video),
        "mp3" | "ogg" | "oga" | "m4a" | "flac" | "wav" | "opus" => Some(MediaKind::Audio),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "svg" => Some(MediaKind::Image),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_video_services() {
        let link = find_media("regarde https://youtu.be/abc?t=10").unwrap();
        assert_eq!(link.kind, MediaKind::YouTube);
        assert_eq!(link.host, "youtu.be");
        assert_eq!(
            find_media("https://vimeo.com/12345").unwrap().kind,
            MediaKind::Vimeo
        );
        assert_eq!(
            find_media("https://streamable.com/abc").unwrap().kind,
            MediaKind::Streamable
        );
    }

    #[test]
    fn detects_direct_media() {
        assert_eq!(
            find_media("https://example.org/clip.mp4").unwrap().kind,
            MediaKind::Video
        );
        assert_eq!(
            find_media("(https://example.org/song.ogg)").unwrap().kind,
            MediaKind::Audio
        );
        assert_eq!(
            find_media("https://example.org/photo.webp").unwrap().kind,
            MediaKind::Image
        );
        assert!(find_media("https://example.org/page.html").is_none());
    }

    #[test]
    fn known_services_require_the_real_host() {
        // Faux positifs historiques : le nom du service apparaissait dans le
        // chemin ou la query, pas dans l'hôte.
        assert!(find_media("https://evil.example/youtube.com/watch").is_none());
        assert!(find_media("https://evil.example/?youtu.be/abc").is_none());
        assert!(find_media("https://youtube.com.evil.example/watch").is_none());
        assert!(find_media("https://notyoutube.com/watch").is_none());
        // Le userinfo ne doit pas tromper l'extraction de l'hôte.
        assert!(find_media("https://youtube.com@evil.example/watch").is_none());
        // Antislash et contrôles de direction : URL ambiguës, refusées.
        assert!(find_media("https://evil.example\\@youtube.com/watch").is_none());
        assert!(find_media("https://evil.example/\u{202e}youtube.com/watch").is_none());
    }

    #[test]
    fn subdomains_and_scheme_case_are_supported() {
        let link = find_media("HTTPS://www.youtube.com/watch?v=abc").unwrap();
        assert_eq!(link.kind, MediaKind::YouTube);
        assert_eq!(link.host, "www.youtube.com");
        assert_eq!(
            find_media("https://m.youtube.com/shorts/x").unwrap().kind,
            MediaKind::YouTube
        );
    }
}
