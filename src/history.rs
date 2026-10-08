use serde::{Deserialize, Serialize};
use std::fs;

use crate::Mode;

const HISTORY_FILE: &str = "history.json";
const HISTORY_TMP: &str = "history.json.tmp";
const HISTORY_BACKUP: &str = "history.json.bak";
const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub mode: String,
    pub status: String,
    pub timestamp: String,
}

pub fn load() -> Vec<HistoryEntry> {
    let Ok(text) = fs::read_to_string(HISTORY_FILE) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<HistoryEntry>>(&text) {
        Ok(entries) => entries,
        Err(_) => {
            let _ = fs::copy(HISTORY_FILE, HISTORY_BACKUP);
            Vec::new()
        }
    }
}

pub fn save(entries: &[HistoryEntry]) {
    if let Ok(text) = serde_json::to_string_pretty(entries) {
        if fs::write(HISTORY_TMP, text).is_ok() && fs::rename(HISTORY_TMP, HISTORY_FILE).is_err() {
            let _ = fs::remove_file(HISTORY_TMP);
        }
    }
}

pub fn record(entries: &mut Vec<HistoryEntry>, url: &str, title: &str, mode: Mode, status: &str) {
    entries.insert(
        0,
        HistoryEntry {
            url: url.to_string(),
            title: title.to_string(),
            mode: match mode {
                Mode::Audio => "Audio".to_string(),
                Mode::Video => "Video".to_string(),
            },
            status: status.to_string(),
            timestamp: now_string(),
        },
    );
    entries.truncate(MAX_ENTRIES);
    save(entries);
}

pub fn clear() {
    let _ = fs::remove_file(HISTORY_FILE);
}

pub fn contains_url(entries: &[HistoryEntry], url: &str) -> bool {
    let Some(id) = video_id(url) else {
        return entries
            .iter()
            .any(|e| e.status == "Completed" && e.url == url);
    };
    entries
        .iter()
        .any(|e| e.status == "Completed" && video_id(&e.url).as_deref() == Some(id.as_str()))
}

pub fn video_id(url: &str) -> Option<String> {
    if let Some(pos) = url.find("youtu.be/") {
        let rest = &url[pos + 9..];
        let id = rest.split(['?', '&', '#', '/']).next().unwrap_or("");
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }

    for marker in [
        "watch?v=",
        "&v=",
        "youtube.com/shorts/",
        "youtube.com/embed/",
        "youtube.com/live/",
    ] {
        if let Some(pos) = url.find(marker) {
            let rest = &url[pos + marker.len()..];
            let id = rest.split(['?', '&', '#', '/']).next().unwrap_or("");
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }

    None
}

fn now_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}