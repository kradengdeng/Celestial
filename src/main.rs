mod downloader;
mod installer;
mod settings;
mod ui;

use crossterm::{
    event::{
        self, Event, KeyCode, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle},
    ExecutableCommand,
};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::sync::Arc;
use std::{
    io::{self, stdout},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use winapi::um::wincon::GetConsoleWindow;
use winapi::um::winuser::{
    LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON,
    LR_DEFAULTSIZE, LR_LOADFROMFILE, WM_SETICON,
};

use crate::downloader::DownloadManager;
use crate::settings::{Settings, ROW_COUNT};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Mode {
    Audio,
    Video,
}

#[derive(Clone, PartialEq)]
pub enum AppScreen {
    Installing,
    Main,
    Settings,
}

#[derive(Clone)]
pub struct DownloadItem {
    pub url: String,
    pub title: Option<String>,
    pub status: DownloadStatus,
}

#[derive(Clone, PartialEq)]
pub enum DownloadStatus {
    Queued,
    Loading,
    Downloading(f32),
    Completed,
    Error(String),
    Cancelled,
}

pub struct AppState {
    pub screen: AppScreen,
    pub install_progress: f32,
    pub mode: Mode,
    pub input_buffer: String,
    pub downloads: Vec<DownloadItem>,
    pub is_dirty: bool,
    pub scroll_offset: usize,
    pub output_path: std::path::PathBuf,
    pub fast_mode: bool,
    pub spinner_frame: usize,
    pub settings: Settings,
    pub settings_cursor: usize,
    pub notice: Option<String>,
    pub notice_busy: bool,
}

pub async fn notify(
    state: &Arc<Mutex<AppState>>,
    tx: &mpsc::Sender<()>,
    text: impl Into<String>,
    busy: bool,
    ttl_ms: Option<u64>,
) {
    let text = text.into();
    {
        let mut s = state.lock().await;
        s.notice = Some(text.clone());
        s.notice_busy = busy;
        s.is_dirty = true;
    }
    let _ = tx.send(()).await;

    if let Some(ms) = ttl_ms {
        let state = state.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let cleared = {
                let mut s = state.lock().await;
                if s.notice.as_deref() == Some(text.as_str()) {
                    s.notice = None;
                    s.notice_busy = false;
                    s.is_dirty = true;
                    true
                } else {
                    false
                }
            };
            if cleared {
                let _ = tx.send(()).await;
            }
        });
    }
}

fn set_console_icon(icon_path: &str) {
    unsafe {
        let hwnd = GetConsoleWindow();
        if hwnd.is_null() {
            return;
        }

        let path_wide: Vec<u16> = OsStr::new(icon_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let hicon = LoadImageW(
            null_mut(),
            path_wide.as_ptr(),
            IMAGE_ICON,
            0,
            0,
            LR_LOADFROMFILE | LR_DEFAULTSIZE,
        );

        if !hicon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, hicon as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, hicon as isize);
        }
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    set_console_icon("app_icon.ico");

    enable_raw_mode()?;
    let mut out = stdout();
    out.execute(EnterAlternateScreen)?;
    out.execute(SetTitle("Celestial"))?;
    let _ = out.execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
    ));

    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = stdout().execute(LeaveAlternateScreen);
        let _ = disable_raw_mode();
        orig_hook(panic_info);
    }));

    let download_dir = std::env::var("USERPROFILE")
        .map(|p| format!("{}\\Downloads", p))
        .unwrap_or_else(|_| ".".to_string());

    let saved = settings::load();
    let output_path = saved
        .output_path
        .filter(|p| std::path::Path::new(p).is_dir())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(download_dir));
    let (audio_slots, video_slots) = (saved.settings.fast_audio_slots, saved.settings.fast_video_slots);

    let state = Arc::new(Mutex::new(AppState {
        screen: AppScreen::Installing,
        install_progress: -1.0,
        mode: saved.mode,
        input_buffer: String::new(),
        downloads: Vec::new(),
        is_dirty: true,
        scroll_offset: 0,
        output_path,
        fast_mode: saved.fast_mode,
        spinner_frame: 0,
        settings: saved.settings,
        settings_cursor: 0,
        notice: None,
        notice_busy: false,
    }));

    let (tx, mut rx) = mpsc::channel(100);
    let mut manager = DownloadManager::new(state.clone(), tx.clone(), audio_slots, video_slots);

    tokio::spawn(installer::check_and_install(state.clone(), tx.clone()));

    let mut last_tick = Instant::now();

    loop {
        manager.poll_expanded().await;

        let mut should_draw = false;
        {
            let mut state_guard = state.lock().await;

            if last_tick.elapsed() >= Duration::from_millis(80) {
                let animating = match state_guard.screen {
                    AppScreen::Installing => true,
                    AppScreen::Settings => false,
                    AppScreen::Main => {
                        state_guard.notice_busy
                            || state_guard.downloads.iter().any(|d| {
                                matches!(d.status, DownloadStatus::Loading | DownloadStatus::Downloading(_))
                            })
                    }
                };
                if animating {
                    state_guard.spinner_frame = state_guard.spinner_frame.wrapping_add(1);
                    state_guard.is_dirty = true;
                }
                last_tick = Instant::now();
            }

            if state_guard.is_dirty {
                should_draw = true;
                state_guard.is_dirty = false;
            }
        }

        if should_draw {
            let state_guard = state.lock().await;
            ui::draw(&state_guard)?;
        }

        while let Ok(_) = rx.try_recv() {}

        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Release {
                    continue;
                }

                if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
                    manager.cancel_all().await;
                    break;
                }

                let screen = { state.lock().await.screen.clone() };

                if screen == AppScreen::Installing {
                    if key.code == KeyCode::Esc {
                        break;
                    }
                    continue;
                }

                if screen == AppScreen::Settings {
                    let mut limits_changed = false;
                    {
                        let mut s = state.lock().await;
                        let mut save = false;
                        match key.code {
                            KeyCode::Esc | KeyCode::F(7) => {
                                s.screen = AppScreen::Main;
                                s.is_dirty = true;
                                save = true;
                            }
                            KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
                                s.settings_cursor = (s.settings_cursor + ROW_COUNT - 1) % ROW_COUNT;
                                s.is_dirty = true;
                            }
                            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                                s.settings_cursor = (s.settings_cursor + 1) % ROW_COUNT;
                                s.is_dirty = true;
                            }
                            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                                let row = s.settings_cursor;
                                s.settings.adjust(row, -1);
                                s.is_dirty = true;
                                limits_changed = true;
                                save = true;
                            }
                            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                                let row = s.settings_cursor;
                                s.settings.adjust(row, 1);
                                s.is_dirty = true;
                                limits_changed = true;
                                save = true;
                            }
                            _ => {}
                        }
                        if save {
                            settings::save(&s);
                        }
                    }
                    if limits_changed {
                        manager.apply_limits().await;
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Esc => {
                        manager.cancel_all().await;
                        break;
                    }
                    KeyCode::F(5) => {
                        let mut s = state.lock().await;
                        s.mode = if s.mode == Mode::Audio { Mode::Video } else { Mode::Audio };
                        s.is_dirty = true;
                        settings::save(&s);
                    }
                    KeyCode::F(6) => {
                        let new_fast = {
                            let mut s = state.lock().await;
                            s.fast_mode = !s.fast_mode;
                            s.is_dirty = true;
                            settings::save(&s);
                            s.fast_mode
                        };
                        manager.on_fast_mode_toggled(new_fast).await;
                    }
                    KeyCode::F(7) => {
                        let mut s = state.lock().await;
                        s.screen = AppScreen::Settings;
                        s.is_dirty = true;
                    }
                    KeyCode::F(8) => {
                        let (has_active, updating) = {
                            let s = state.lock().await;
                            (
                                s.downloads.iter().any(|d| {
                                    matches!(d.status, DownloadStatus::Loading | DownloadStatus::Downloading(_))
                                }),
                                s.notice_busy,
                            )
                        };
                        if !updating {
                            if has_active {
                                notify(&state, &tx, "Stop active downloads before updating yt-dlp", false, Some(4000)).await;
                            } else {
                                let state_clone = state.clone();
                                let tx_clone = tx.clone();
                                tokio::spawn(async move {
                                    notify(&state_clone, &tx_clone, "Updating yt-dlp…", true, None).await;
                                    let msg = downloader::update_yt_dlp().await;
                                    notify(&state_clone, &tx_clone, msg, false, Some(8000)).await;
                                });
                            }
                        }
                    }
                    KeyCode::Up => {
                        let mut s = state.lock().await;
                        if s.scroll_offset > 0 {
                            s.scroll_offset -= 1;
                            s.is_dirty = true;
                        }
                    }
                    KeyCode::Down => {
                        let mut s = state.lock().await;
                        let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                        let available = ui::list_capacity(rows);
                        if s.scroll_offset + available < s.downloads.len() {
                            s.scroll_offset += 1;
                            s.is_dirty = true;
                        }
                    }
                    KeyCode::Tab => {
                        let tx_clone = tx.clone();
                        let state_clone = state.clone();
                        tokio::spawn(async move {
                            let ps_script = r#"Add-Type -AssemblyName System.windows.forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.AutoUpgradeEnabled = $true; $f.ShowNewFolderButton = $true; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.SelectedPath }"#;
                            let output = tokio::process::Command::new("powershell")
                                .args(&["-NoProfile", "-STA", "-Command", ps_script])
                                .output()
                                .await;
                            if let Ok(out) = output {
                                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                                if !path.is_empty() {
                                    let mut s = state_clone.lock().await;
                                    s.output_path = std::path::PathBuf::from(path);
                                    s.is_dirty = true;
                                    settings::save(&s);
                                    drop(s);
                                    let _ = tx_clone.send(()).await;
                                }
                            }
                        });
                    }
                    KeyCode::Char(' ') => {
                        let has_active = {
                            let state_guard = state.lock().await;
                            state_guard
                                .downloads
                                .iter()
                                .any(|d| matches!(d.status, DownloadStatus::Loading | DownloadStatus::Downloading(_)))
                        };
                        if has_active {
                            manager.cancel_all().await;
                        } else {
                            let mut state_guard = state.lock().await;
                            state_guard.input_buffer.push(' ');
                            state_guard.is_dirty = true;
                        }
                    }
                    KeyCode::Char('v') | KeyCode::Char('V')
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        let text = arboard::Clipboard::new()
                            .and_then(|mut c| c.get_text())
                            .unwrap_or_default();
                        let lines: Vec<&str> = text
                            .lines()
                            .map(|l| l.trim())
                            .filter(|l| !l.is_empty())
                            .collect();
                        match lines.len() {
                            0 => {
                                notify(&state, &tx, "Clipboard has no text", false, Some(3000)).await;
                            }
                            1 => {
                                let mut s = state.lock().await;
                                s.input_buffer.push_str(lines[0]);
                                s.is_dirty = true;
                            }
                            n => {
                                for line in &lines {
                                    manager.submit(line.to_string()).await;
                                }
                                notify(&state, &tx, format!("Added {} links from clipboard", n), false, Some(4000)).await;
                            }
                        }
                    }
                    KeyCode::Char(c) => {
                        let mut state_guard = state.lock().await;
                        state_guard.input_buffer.push(c);
                        state_guard.is_dirty = true;
                    }
                    KeyCode::Backspace => {
                        let mut state_guard = state.lock().await;
                        if state_guard.input_buffer.pop().is_some() {
                            state_guard.is_dirty = true;
                        }
                    }
                    KeyCode::Enter => {
                        let input = {
                            let mut state_guard = state.lock().await;
                            let val = state_guard.input_buffer.clone();
                            state_guard.input_buffer.clear();
                            state_guard.is_dirty = true;
                            val
                        };
                        let input_trimmed = input.trim();
                        if !input_trimmed.is_empty() {
                            manager.submit(input_trimmed.to_string()).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = stdout().execute(PopKeyboardEnhancementFlags);
    stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}