//! Détection des liens média dans les messages (vidéo, audio, image).
//!
//! GPUI n'intègre pas de moteur web : le client affiche un bouton « ▶ » qui
//! ouvre le lien dans le navigateur/lecteur du système. Les services connus
//! sont reconnus pour adapter le libellé.

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
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        if let Some(kind) = classify(url) {
            return Some(MediaLink {
                url: url.to_string(),
                kind,
            });
        }
    }
    None
}

fn classify(url: &str) -> Option<MediaKind> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("youtube.com/watch")
        || lower.contains("youtu.be/")
        || lower.contains("youtube.com/shorts/")
        || lower.contains("youtube.com/live/")
    {
        return Some(MediaKind::YouTube);
    }
    if lower.contains("vimeo.com/") {
        return Some(MediaKind::Vimeo);
    }
    if lower.contains("dailymotion.com/video/") || lower.contains("dai.ly/") {
        return Some(MediaKind::Dailymotion);
    }
    if lower.contains("streamable.com/") {
        return Some(MediaKind::Streamable);
    }
    let path = lower
        .split(['?', '#'])
        .next()
        .unwrap_or(&lower)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    match extension {
        Some("mp4" | "webm" | "m3u8" | "mov" | "mkv") => Some(MediaKind::Video),
        Some("mp3" | "ogg" | "oga" | "m4a" | "flac" | "wav" | "opus") => Some(MediaKind::Audio),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "svg") => {
            Some(MediaKind::Image)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_video_services() {
        assert_eq!(
            find_media("regarde https://youtu.be/abc?t=10")
                .unwrap()
                .kind,
            MediaKind::YouTube
        );
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
}
