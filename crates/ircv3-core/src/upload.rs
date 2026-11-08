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
    if !status.is_success() || !(text.starts_with("http://") || text.starts_with("https://")) {
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
}
