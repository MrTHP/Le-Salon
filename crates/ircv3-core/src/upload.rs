//! Téléversement de fichiers sur litterbox.catbox.moe (public, 72 h).
//!
//! IRC ne transporte pas de fichiers : on téléverse puis on poste le lien.
//! Utilisé par le bouton « + » et la commande `/upload`.

use std::{fs, path::Path, time::Duration};

use crate::i18n::t;

pub const UPLOAD_ENDPOINT: &str = "https://litterbox.catbox.moe/resources/internals/api.php";
pub const UPLOAD_MAX: u64 = 50 * 1024 * 1024;

pub fn upload_file(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|error| {
        if crate::i18n::lang() == crate::i18n::Lang::En {
            format!("cannot read file: {error}")
        } else {
            format!("fichier illisible : {error}")
        }
    })?;
    if !metadata.is_file() {
        return Err(t("Ce n'est pas un fichier.", "This is not a file.").into());
    }
    if metadata.len() > UPLOAD_MAX {
        return Err(t(
            "Fichier trop lourd (maximum 50 Mo).",
            "File too large (50 MB maximum).",
        )
        .into());
    }
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("fichier")
        .to_string();
    let bytes = fs::read(path).map_err(|error| {
        if crate::i18n::lang() == crate::i18n::Lang::En {
            format!("cannot read file: {error}")
        } else {
            format!("lecture impossible : {error}")
        }
    })?;
    let bytes = sanitize_image(bytes)?;
    let part = reqwest::blocking::multipart::Part::bytes(bytes).file_name(filename.clone());
    let form = reqwest::blocking::multipart::Form::new()
        .text("reqtype", "fileupload")
        .text("time", "72h")
        .part("fileToUpload", part);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| format!("HTTP client: {error}"))?;
    let response = client
        .post(UPLOAD_ENDPOINT)
        .multipart(form)
        .send()
        .map_err(|error| {
            if crate::i18n::lang() == crate::i18n::Lang::En {
                format!("upload failed: {error}")
            } else {
                format!("échec du téléversement : {error}")
            }
        })?;
    let status = response.status();
    let text = response.text().unwrap_or_default().trim().to_string();
    // Un lien public doit être HTTPS : on refuse un retour en clair.
    if !status.is_success() || !text.starts_with("https://") {
        return Err(if crate::i18n::lang() == crate::i18n::Lang::En {
            format!(
                "server response {status}: {}",
                text.chars().take(160).collect::<String>()
            )
        } else {
            format!(
                "réponse {status} : {}",
                text.chars().take(160).collect::<String>()
            )
        });
    }
    Ok(text)
}

/// Retire les métadonnées des images JPEG et PNG avant un upload public.
///
/// Les autres formats sont renvoyés tels quels. En cas d'erreur sur un format
/// reconnu, l'upload est refusé plutôt que de risquer de publier des données
/// EXIF/XMP (GPS, appareil, logiciel, commentaires…).
fn sanitize_image(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.starts_with(&[0xff, 0xd8]) {
        return strip_jpeg_metadata(&bytes).ok_or_else(|| {
            t(
                "Impossible de retirer les métadonnées de cette image JPEG.",
                "Could not remove metadata from this JPEG image.",
            )
            .to_string()
        });
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return strip_png_metadata(&bytes).ok_or_else(|| {
            t(
                "Impossible de retirer les métadonnées de cette image PNG.",
                "Could not remove metadata from this PNG image.",
            )
            .to_string()
        });
    }
    Ok(bytes)
}

fn strip_jpeg_metadata(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(&bytes[..2]);
    let mut index = 2;
    while index + 1 < bytes.len() {
        if bytes[index] != 0xff {
            return None;
        }
        let marker = bytes[index + 1];
        if marker == 0xd9 {
            output.extend_from_slice(&bytes[index..]);
            return Some(output);
        }
        if marker == 0xda {
            // Début des données compressées : on conserve toute la fin.
            output.extend_from_slice(&bytes[index..]);
            return Some(output);
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            output.extend_from_slice(&bytes[index..index + 2]);
            index += 2;
            continue;
        }
        if index + 4 > bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 || index + 2 + length > bytes.len() {
            return None;
        }
        let segment = &bytes[index..index + 2 + length];
        let is_metadata = marker == 0xe1 // EXIF / XMP
            || marker == 0xed // IPTC / Photoshop
            || marker == 0xfe; // commentaire
        if !is_metadata {
            output.extend_from_slice(segment);
        }
        index += 2 + length;
    }
    None
}

fn strip_png_metadata(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(bytes.len());
    output.extend_from_slice(&bytes[..8]);
    let mut index = 8;
    while index + 12 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[index],
            bytes[index + 1],
            bytes[index + 2],
            bytes[index + 3],
        ]) as usize;
        let end = index.checked_add(12)?.checked_add(length)?;
        if end > bytes.len() {
            return None;
        }
        let kind = &bytes[index + 4..index + 8];
        let is_metadata = matches!(kind, b"tEXt" | b"iTXt" | b"zTXt" | b"eXIf" | b"tIME");
        if !is_metadata {
            output.extend_from_slice(&bytes[index..end]);
        }
        index = end;
        if kind == b"IEND" {
            return Some(output);
        }
    }
    None
}

pub fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    let (g, m, k, o) = if crate::i18n::lang() == crate::i18n::Lang::En {
        ("GB", "MB", "KB", "B")
    } else {
        ("Go", "Mo", "Ko", "o")
    };
    if value >= GIB {
        format!("{:.2} {g}", value / GIB)
    } else if value >= MIB {
        format!("{:.1} {m}", value / MIB)
    } else if value >= KIB {
        format!("{:.1} {k}", value / KIB)
    } else {
        format!("{bytes} {o}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(512), "512 o");
        assert_eq!(format_size(1536), "1.5 Ko");
        assert_eq!(format_size(2 * 1024 * 1024), "2.0 Mo");
    }

    #[test]
    fn strips_jpeg_metadata() {
        let bytes = vec![
            0xff, 0xd8, // SOI
            0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, // APP0
            0xff, 0xe1, 0x00, 0x06, b'E', b'X', b'I', b'F', // APP1
            0xff, 0xda, 0x00, 0x02, // SOS
            0xff, 0xd9, // EOI
        ];
        let stripped = sanitize_image(bytes).unwrap();
        assert!(stripped.windows(2).any(|window| window == [0xff, 0xe0]));
        assert!(!stripped.windows(2).any(|window| window == [0xff, 0xe1]));
        assert!(stripped.ends_with(&[0xff, 0xd9]));
    }

    #[test]
    fn strips_png_text_chunks() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0u8; 13]);
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&4u32.to_be_bytes());
        bytes.extend_from_slice(b"tEXt");
        bytes.extend_from_slice(b"test");
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(b"IDAT");
        bytes.extend_from_slice(&[0u8; 4]);
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(b"IEND");
        bytes.extend_from_slice(&[0u8; 4]);

        let stripped = sanitize_image(bytes).unwrap();
        assert!(!stripped.windows(4).any(|window| window == b"tEXt"));
        assert!(stripped.windows(4).any(|window| window == b"IHDR"));
        assert!(stripped.windows(4).any(|window| window == b"IDAT"));
        assert!(stripped.windows(4).any(|window| window == b"IEND"));
    }
}
