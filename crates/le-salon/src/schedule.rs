//! Messages planifiés, persistés dans `scheduled.json`.
//!
//! Syntaxes acceptées :
//!   `/schedule +10m texte`
//!   `/schedule 18:30 texte`
//!   `/schedule 2026-11-08 09:00 texte`

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

use ircv3_core::i18n::t;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledItem {
    pub id: u64,
    pub network: String,
    pub target: String,
    pub text: String,
    /// RFC 3339 en heure locale.
    pub due: String,
}

impl ScheduledItem {
    pub fn due_at(&self) -> Option<DateTime<Local>> {
        DateTime::parse_from_rfc3339(&self.due)
            .ok()
            .map(|date| date.with_timezone(&Local))
    }

    pub fn due_label(&self) -> String {
        self.due_at()
            .map(|date| date.format("%d/%m %H:%M").to_string())
            .unwrap_or_else(|| self.due.clone())
    }
}

pub struct ScheduleStore {
    pub items: Vec<ScheduledItem>,
    next_id: u64,
    path: PathBuf,
}

impl ScheduleStore {
    pub fn load() -> Self {
        let path = ircv3_core::Config::path()
            .parent()
            .map(|parent| parent.join("scheduled.json"))
            .unwrap_or_else(|| PathBuf::from("scheduled.json"));
        if !path.exists() {
            let legacy = path
                .parent()
                .and_then(|app_dir| app_dir.parent())
                .map(|config_root| config_root.join("ircv3-gpui").join("scheduled.json"));
            if let Some(legacy) = legacy {
                if legacy.exists() {
                    if let Some(parent) = path.parent() {
                        let _ = fs::create_dir_all(parent);
                        restrict_dir(parent);
                    }
                    let _ = fs::copy(&legacy, &path);
                    restrict_file(&path);
                }
            }
        }
        let items = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<ScheduledItem>>(&text).ok())
            .unwrap_or_default();
        let next_id = items.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        Self {
            items,
            next_id,
            path,
        }
    }

    pub fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
            restrict_dir(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.items) {
            if let Err(error) = write_private(&self.path, &text) {
                log::warn!("sauvegarde des messages planifiés impossible : {error}");
            }
        }
    }

    pub fn add(
        &mut self,
        network: String,
        target: String,
        text: String,
        due: DateTime<Local>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(ScheduledItem {
            id,
            network,
            target,
            text,
            due: due.to_rfc3339(),
        });
        self.items.sort_by_key(|item| item.due.clone());
        self.save();
        id
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        let changed = self.items.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    pub fn take_due(&mut self, now: DateTime<Local>) -> Vec<ScheduledItem> {
        let mut due = Vec::new();
        let mut remaining = Vec::new();
        for item in self.items.drain(..) {
            if item.due_at().is_none_or(|at| at <= now) {
                due.push(item);
            } else {
                remaining.push(item);
            }
        }
        self.items = remaining;
        if !due.is_empty() {
            self.save();
        }
        due
    }
}

fn restrict_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
}

fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
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
        // `mode` ne s'applique qu'à la création : on force aussi le mode si
        // le fichier existait déjà avec des permissions trop larges.
        restrict_file(path);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, text)
    }
}

pub fn parse_command(input: &str) -> Result<(String, DateTime<Local>), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err(t(
            "Usage : /schedule +10m texte, /schedule 18:30 texte ou /schedule 2026-11-08 09:00 texte",
            "Usage: /schedule +10m text, /schedule 18:30 text or /schedule 2026-11-08 09:00 text",
        )
        .into());
    }
    let now = Local::now();
    if let Some(rest) = input.strip_prefix('+') {
        let (duration, text) = split_first(rest);
        let due = now + parse_duration(duration)?;
        if text.is_empty() {
            return Err(t("Texte du message manquant.", "Message text missing.").into());
        }
        return Ok((format!("{text}"), due));
    }
    // Date complète YYYY-MM-DD HH:MM
    if input.len() >= 16 {
        let date_part = input.get(..10);
        let time_part = input.get(11..16);
        if let (Some(date_part), Some(time_part)) = (date_part, time_part) {
            if let (Ok(date), Ok(time)) = (
                NaiveDate::parse_from_str(date_part, "%Y-%m-%d"),
                NaiveTime::parse_from_str(time_part, "%H:%M"),
            ) {
                if let Some(due) = Local
                    .from_local_datetime(&date.and_time(time))
                    .single()
                    .or_else(|| Local.from_local_datetime(&date.and_time(time)).earliest())
                {
                    let text = input.get(16..).unwrap_or("").trim();
                    if text.is_empty() {
                        return Err(t("Texte du message manquant.", "Message text missing.").into());
                    }
                    return Ok((text.to_string(), due));
                }
            }
        }
    }
    // Heure HH:MM (aujourd'hui ou demain)
    if input.len() >= 5 {
        if let Some(hhmm) = input.get(..5)
            && let Ok(time) = NaiveTime::parse_from_str(hhmm, "%H:%M")
        {
            let mut date = now.date_naive();
            let mut due = Local
                .from_local_datetime(&date.and_time(time))
                .single()
                .unwrap_or(now);
            if due <= now {
                date += Duration::days(1);
                due = Local
                    .from_local_datetime(&date.and_time(time))
                    .single()
                    .unwrap_or(now);
            }
            let text = input.get(5..).unwrap_or("").trim();
            if text.is_empty() {
                return Err(t("Texte du message manquant.", "Message text missing.").into());
            }
            return Ok((text.to_string(), due));
        }
    }
    Err(t(
        "Format de date invalide. Exemples : +10m, 18:30, 2026-11-08 09:00.",
        "Invalid date format. Examples: +10m, 18:30, 2026-11-08 09:00.",
    )
    .into())
}

fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    let mut chars = value.chars();
    let unit = chars.next_back().ok_or_else(|| {
        t(
            "Durée invalide (exemples : +10m, +2h, +30s).",
            "Invalid duration (examples: +10m, +2h, +30s).",
        )
        .to_string()
    })?;
    let number = &value[..value.len() - unit.len_utf8()];
    let number: i64 = number.parse().map_err(|_| {
        t(
            "Durée invalide (exemples : +10m, +2h, +30s).",
            "Invalid duration (examples: +10m, +2h, +30s).",
        )
        .to_string()
    })?;
    match unit {
        's' => Ok(Duration::seconds(number)),
        'm' => Ok(Duration::minutes(number)),
        'h' => Ok(Duration::hours(number)),
        'd' => Ok(Duration::days(number)),
        _ => Err(t(
            "Unité de durée invalide (s, m, h ou d).",
            "Invalid duration unit (s, m, h or d).",
        )
        .into()),
    }
}

fn split_first(value: &str) -> (&str, &str) {
    match value.split_once(char::is_whitespace) {
        Some((first, rest)) => (first, rest.trim_start()),
        None => (value, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_duration() {
        let (text, due) = parse_command("+10m bonjour").unwrap();
        assert_eq!(text, "bonjour");
        assert!(due > Local::now());
    }

    #[test]
    fn parses_absolute_time() {
        let (text, _) = parse_command("18:30 bonsoir").unwrap();
        assert_eq!(text, "bonsoir");
    }

    #[test]
    fn parses_full_date() {
        let (text, _) = parse_command("2026-11-08 09:00 demain").unwrap();
        assert_eq!(text, "demain");
    }

    #[cfg(unix)]
    #[test]
    fn scheduled_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "le-salon-schedule-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scheduled.json");
        write_private(&path, "[]").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(dir);
    }
}
