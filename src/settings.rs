use crossterm::style::Color;
use serde::{Deserialize, Serialize};

use crate::{AppState, Mode};

pub const CONFIG_FILE: &str = "settings.json";

pub const AUDIO_FORMATS: [&str; 5] = ["mp3", "m4a", "opus", "flac", "wav"];
pub const AUDIO_QUALITIES: [&str; 5] = ["Best", "320K", "256K", "192K", "128K"];
pub const VIDEO_HEIGHTS: [u32; 5] = [0, 1080, 720, 480, 360];
pub const FAST_DOWNLOAD_STEPS: [usize; 6] = [5, 10, 15, 20, 25, 30];

pub const ACCENTS: [(&str, (u8, u8, u8)); 8] = [
    ("Light Purple", (216, 134, 255)),
    ("Light Blue", (125, 196, 255)),
    ("Light Cyan", (128, 232, 232)),
    ("Light Green", (144, 238, 144)),
    ("Light Yellow", (255, 236, 139)),
    ("Light Orange", (255, 184, 108)),
    ("Light Pink", (255, 150, 200)),
    ("Light Red", (255, 130, 130)),
];

pub const COOKIE_SOURCES: [&str; 6] =
    ["Off", "Firefox", "Chrome", "Edge", "Brave", "cookies.txt"];

pub const FAST_DOWNLOADS_ROW: usize = 6;
pub const DUPLICATE_DETECTION_ROW: usize = 7;
pub const AUTO_FILENAME_ROW: usize = 8;
pub const ACCENT_ROW: usize = 9;
pub const NOTIFY_SOUND_ROW: usize = 10;
pub const AUTO_UPDATE_ROW: usize = 11;
pub const UPDATE_YTDLP_ROW: usize = 12;
pub const UPDATE_CELESTIAL_ROW: usize = 13;
pub const ROW_COUNT: usize = 14;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub audio_format: String,
    pub audio_quality: String,
    pub video_height: u32,
    pub embed_meta: bool,
    pub expand_playlists: bool,
    pub fast_downloads: usize,
    pub duplicate_detection: bool,
    pub auto_filename: bool,
    pub accent: String,
    pub notify_sound: bool,
    pub cookies: String,
    pub auto_update: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            audio_format: "mp3".to_string(),
            audio_quality: "Best".to_string(),
            video_height: 0,
            embed_meta: false,
            expand_playlists: true,
            fast_downloads: 5,
            duplicate_detection: false,
            auto_filename: false,
            accent: ACCENTS[0].0.to_string(),
            notify_sound: false,
            cookies: "Off".to_string(),
            auto_update: false,
        }
    }
}

fn cycle(len: usize, pos: Option<usize>, dir: i32) -> usize {
    let p = pos.unwrap_or(0) as i32;
    (p + dir).rem_euclid(len as i32) as usize
}

impl Settings {
    fn sanitize(&mut self) {
        if !AUDIO_FORMATS.contains(&self.audio_format.as_str()) {
            self.audio_format = "mp3".to_string();
        }

        if !AUDIO_QUALITIES.contains(&self.audio_quality.as_str()) {
            self.audio_quality = "Best".to_string();
        }

        if !VIDEO_HEIGHTS.contains(&self.video_height) {
            self.video_height = 0;
        }

        if !ACCENTS.iter().any(|(name, _)| *name == self.accent) {
            self.accent = ACCENTS[0].0.to_string();
        }

        if !COOKIE_SOURCES.contains(&self.cookies.as_str()) {
            self.cookies = "Off".to_string();
        }

        if !FAST_DOWNLOAD_STEPS.contains(&self.fast_downloads) {
            self.fast_downloads = 5;
        }
    }

    pub fn adjust(&mut self, row: usize, dir: i32) {
        match row {
            0 => {
                let i = cycle(
                    AUDIO_FORMATS.len(),
                    AUDIO_FORMATS.iter().position(|f| *f == self.audio_format),
                    dir,
                );
                self.audio_format = AUDIO_FORMATS[i].to_string();
            }
            1 => {
                let i = cycle(
                    AUDIO_QUALITIES.len(),
                    AUDIO_QUALITIES
                        .iter()
                        .position(|q| *q == self.audio_quality),
                    dir,
                );
                self.audio_quality = AUDIO_QUALITIES[i].to_string();
            }
            2 => {
                let i = cycle(
                    VIDEO_HEIGHTS.len(),
                    VIDEO_HEIGHTS.iter().position(|h| *h == self.video_height),
                    dir,
                );
                self.video_height = VIDEO_HEIGHTS[i];
            }
            3 => self.embed_meta = !self.embed_meta,
            4 => self.expand_playlists = !self.expand_playlists,
            5 => {
                let i = cycle(
                    COOKIE_SOURCES.len(),
                    COOKIE_SOURCES.iter().position(|c| *c == self.cookies),
                    dir,
                );
                self.cookies = COOKIE_SOURCES[i].to_string();
            }
            FAST_DOWNLOADS_ROW => {
                let i = cycle(
                    FAST_DOWNLOAD_STEPS.len(),
                    FAST_DOWNLOAD_STEPS
                        .iter()
                        .position(|n| *n == self.fast_downloads),
                    dir,
                );
                self.fast_downloads = FAST_DOWNLOAD_STEPS[i];
            }
            DUPLICATE_DETECTION_ROW => self.duplicate_detection = !self.duplicate_detection,
            AUTO_FILENAME_ROW => self.auto_filename = !self.auto_filename,
            ACCENT_ROW => {
                let i = cycle(
                    ACCENTS.len(),
                    ACCENTS.iter().position(|(name, _)| *name == self.accent),
                    dir,
                );
                self.accent = ACCENTS[i].0.to_string();
            }
            NOTIFY_SOUND_ROW => self.notify_sound = !self.notify_sound,
            AUTO_UPDATE_ROW => self.auto_update = !self.auto_update,
            UPDATE_YTDLP_ROW | UPDATE_CELESTIAL_ROW => {}
            _ => {}
        }
    }

    pub fn accent_color(&self) -> Color {
        let (r, g, b) = ACCENTS
            .iter()
            .find(|(name, _)| *name == self.accent)
            .map(|(_, rgb)| *rgb)
            .unwrap_or(ACCENTS[0].1);

        Color::Rgb { r, g, b }
    }

    pub fn rows(&self) -> Vec<(&'static str, String, &'static str)> {
        vec![
            (
                "Audio format",
                self.audio_format.to_uppercase(),
                "Output format in audio mode",
            ),
            (
                "Audio quality",
                self.audio_quality.clone(),
                "MP3 / M4A / OPUS only (lossless ignores it)",
            ),
            (
                "Video quality",
                if self.video_height == 0 {
                    "Best".to_string()
                } else {
                    format!("{}p", self.video_height)
                },
                "Maximum resolution in video mode",
            ),
            (
                "Cover art & tags",
                if self.embed_meta {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                "Embed thumbnail and metadata (slower)",
            ),
            (
                "Playlist links",
                if self.expand_playlists {
                    "Expand all".to_string()
                } else {
                    "Single video".to_string()
                },
                "What to do with playlist URLs",
            ),
            (
                "Cookies",
                self.cookies.clone(),
                "Fixes bot check, age and login errors",
            ),
            (
                "Fast downloads",
                self.fast_downloads.to_string(),
                "Maximum simultaneous Fast downloads",
            ),
            (
                "Duplicate detection",
                if self.duplicate_detection {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                "Skip videos already in download history",
            ),
            (
                "Auto filename",
                if self.auto_filename {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                "Make filenames Windows-safe and shorter",
            ),
            (
                "Accent color",
                self.accent.clone(),
                "Highlight color of the interface",
            ),
            (
                "Success sound",
                if self.notify_sound {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                "Play a sound when all downloads finish",
            ),
            (
                "Auto update",
                if self.auto_update {
                    "On".to_string()
                } else {
                    "Off".to_string()
                },
                "Install new Celestial versions on startup",
            ),
            (
                "Update yt-dlp",
                "Press Enter".to_string(),
                "Download the latest yt-dlp version",
            ),
            (
                "Update Celestial",
                "Press Enter".to_string(),
                "Check GitHub for a new Celestial release",
            ),
        ]
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct SavedConfig {
    settings: Settings,
    mode: Mode,
    fast_mode: bool,
    output_path: Option<String>,
}

impl Default for SavedConfig {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            mode: Mode::Audio,
            fast_mode: false,
            output_path: None,
        }
    }
}

pub struct Loaded {
    pub settings: Settings,
    pub mode: Mode,
    pub fast_mode: bool,
    pub output_path: Option<String>,
}

pub fn load() -> Loaded {
    let cfg: SavedConfig = std::fs::read_to_string(CONFIG_FILE)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut settings = cfg.settings;
    settings.sanitize();

    Loaded {
        settings,
        mode: cfg.mode,
        fast_mode: cfg.fast_mode,
        output_path: cfg.output_path,
    }
}

pub fn save(state: &AppState) {
    let cfg = SavedConfig {
        settings: state.settings.clone(),
        mode: state.mode,
        fast_mode: state.fast_mode,
        output_path: Some(state.output_path.to_string_lossy().to_string()),
    };

    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(CONFIG_FILE, json);
    }
}