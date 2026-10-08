mod downloader;
mod history;
mod installer;
mod settings;
mod ui;
mod updater;

use crossterm::{
    event::{
        self, Event, KeyCode, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    cursor::Show,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
    ExecutableCommand,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    io::{self, stdout},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use crate::downloader::DownloadManager;
use crate::settings::{
    Settings, FAST_DOWNLOADS_ROW, ROW_COUNT, UPDATE_CELESTIAL_ROW, UPDATE_YTDLP_ROW,
};

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
    History,
    UpdatePrompt,
    Updating,
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

#[derive(Clone, Copy, PartialEq)]
pub enum HistoryConfirm {
    Delete,
    Clear,
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
    pub history: Vec<history::HistoryEntry>,
    pub history_cursor: usize,
    pub history_confirm: Option<HistoryConfirm>,
    pub notice: Option<String>,
    pub notice_busy: bool,
    pub pending_update: Option<updater::Release>,
    pub loading_text: Option<String>,
    pub batch_completed: usize,
    pub quit: bool,
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

fn restore_terminal() {
    let mut out = stdout();
    let _ = out.execute(PopKeyboardEnhancementFlags);
    let _ = out.execute(Show);
    let _ = out.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    {
        let _ = std::env::set_current_dir(dir);
    }

    updater::cleanup();

    enable_raw_mode()?;
    let _terminal_guard = TerminalGuard;
    let mut out = stdout();
    out.execute(EnterAlternateScreen)?;
    out.execute(SetTitle("Celestial"))?;
    let _ = out.execute(PushKeyboardEnhancementFlags(
        KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES,
    ));

    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        if std::thread::current().name() == Some("main") {
            restore_terminal();
        }
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
    let fast_slots = saved.settings.fast_downloads;
    let history_entries = history::load();

    let state = Arc::new(Mutex::new(AppState {
        screen: AppScreen::Installing,
        install_progress: 0.0,
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
        history: history_entries,
        history_cursor: 0,
        history_confirm: None,
        notice: None,
        notice_busy: false,
        pending_update: None,
        loading_text: None,
        batch_completed: 0,
        quit: false,
    }));

    let (tx, mut rx) = mpsc::channel::<()>(100);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let mut manager = DownloadManager::new(state.clone(), tx.clone(), fast_slots);

    tokio::spawn(installer::check_and_install(state.clone(), tx.clone()));

    let mut last_tick = Instant::now();

    loop {
        manager.poll_expanded().await;

        let mut should_draw = false;
        {
            let mut state_guard = state.lock().await;

            if last_tick.elapsed() >= Duration::from_millis(80) {
                let animating = match state_guard.screen {
                    AppScreen::Installing | AppScreen::Updating => true,
                    AppScreen::Settings | AppScreen::History => state_guard.notice_busy,
                    AppScreen::UpdatePrompt => false,
                    AppScreen::Main => {
                        state_guard.notice_busy
                            || state_guard.downloads.iter().any(|d| {
                                matches!(
                                    d.status,
                                    DownloadStatus::Loading | DownloadStatus::Downloading(_)
                                )
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

        if state.lock().await.quit {
            break;
        }

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

                if screen == AppScreen::Updating {
                    continue;
                }
                if screen == AppScreen::UpdatePrompt {
                    match key.code {
                        KeyCode::Char('u') | KeyCode::Char('U') => {
                            let release = { state.lock().await.pending_update.clone() };
                            if let Some(release) = release {
                                tokio::spawn(updater::apply(state.clone(), tx.clone(), release));
                            }
                        }
                        KeyCode::Enter
                        | KeyCode::Esc
                        | KeyCode::Char('n')
                        | KeyCode::Char('N')
                        | KeyCode::Char(' ') => {
                            let mut s = state.lock().await;
                            s.screen = AppScreen::Main;
                            s.pending_update = None;
                            s.is_dirty = true;
                        }
                        _ => {}
                    }
                    continue;
                }

                if screen == AppScreen::History {
                    let mut history_action: Option<HistoryConfirm> = None;

                    {
                        let mut s = state.lock().await;

                        if let Some(confirm) = s.history_confirm {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    history_action = Some(confirm);
                                    s.history_confirm = None;
                                    s.notice = None;
                                    s.notice_busy = false;
                                    s.is_dirty = true;
                                }
                                KeyCode::Char('n')
                                | KeyCode::Char('N')
                                | KeyCode::Esc => {
                                    s.history_confirm = None;
                                    s.notice = None;
                                    s.notice_busy = false;
                                    s.is_dirty = true;
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Esc | KeyCode::F(4) => {
                                    s.screen = AppScreen::Main;
                                    s.is_dirty = true;
                                }
                                KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
                                    if s.history_cursor > 0 {
                                        s.history_cursor -= 1;
                                        s.is_dirty = true;
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                                    if s.history_cursor + 1 < s.history.len() {
                                        s.history_cursor += 1;
                                        s.is_dirty = true;
                                    }
                                }
                                KeyCode::Char('f') | KeyCode::Char('F') => {
                                    if let Some(entry) = s.history.get(s.history_cursor) {
                                        let url = entry.url.clone();
                                        s.screen = AppScreen::Main;
                                        s.input_buffer.clear();
                                        s.is_dirty = true;
                                        drop(s);
                                        manager.resubmit(url).await;
                                        continue;
                                    }
                                }
                                KeyCode::Char('x') | KeyCode::Char('X') => {
                                    if !s.history.is_empty() {
                                        s.history_confirm = Some(HistoryConfirm::Delete);
                                        s.notice = Some("Delete selected history entry? [Y/N]".to_string());
                                        s.notice_busy = false;
                                        s.is_dirty = true;
                                    }
                                }
                                KeyCode::Char('c') | KeyCode::Char('C') => {
                                    if !s.history.is_empty() {
                                        s.history_confirm = Some(HistoryConfirm::Clear);
                                        s.notice = Some("Clear all download history? [Y/N]".to_string());
                                        s.notice_busy = false;
                                        s.is_dirty = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    if let Some(action) = history_action {
                        let message = match action {
                            HistoryConfirm::Delete => {
                                let mut s = state.lock().await;
                                if s.history_cursor < s.history.len() {
                                    let cursor = s.history_cursor;
                                    s.history.remove(cursor);
                                    if s.history_cursor >= s.history.len() {
                                        s.history_cursor = s.history.len().saturating_sub(1);
                                    }
                                    history::save(&s.history);
                                    s.is_dirty = true;
                                    Some("History entry deleted")
                                } else {
                                    None
                                }
                            }
                            HistoryConfirm::Clear => {
                                let mut s = state.lock().await;
                                s.history.clear();
                                s.history_cursor = 0;
                                history::clear();
                                s.is_dirty = true;
                                Some("Download history cleared")
                            }
                        };
                        if let Some(message) = message {
                            notify(&state, &tx, message, false, Some(3000)).await;
                        }
                    }

                    continue;
                }

                if screen == AppScreen::Settings {
                    let mut limits_changed = false;
                    let mut update_ytdlp = false;
                    let mut update_celestial = false;

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
                                s.settings_cursor =
                                    (s.settings_cursor + ROW_COUNT - 1) % ROW_COUNT;
                                s.is_dirty = true;
                            }
                            KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
                                s.settings_cursor = (s.settings_cursor + 1) % ROW_COUNT;
                                s.is_dirty = true;
                            }
                            KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
                                let row = s.settings_cursor;
                                if row != UPDATE_YTDLP_ROW && row != UPDATE_CELESTIAL_ROW {
                                    s.settings.adjust(row, -1);
                                    s.is_dirty = true;
                                    save = true;
                                    if row == FAST_DOWNLOADS_ROW {
                                        limits_changed = true;
                                    }
                                }
                            }
                            KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
                                let row = s.settings_cursor;
                                if row != UPDATE_YTDLP_ROW && row != UPDATE_CELESTIAL_ROW {
                                    s.settings.adjust(row, 1);
                                    s.is_dirty = true;
                                    save = true;
                                    if row == FAST_DOWNLOADS_ROW {
                                        limits_changed = true;
                                    }
                                }
                            }
                            KeyCode::Enter => {
                                match s.settings_cursor {
                                    UPDATE_YTDLP_ROW => update_ytdlp = true,
                                    UPDATE_CELESTIAL_ROW => update_celestial = true,
                                    _ => {
                                        let row = s.settings_cursor;
                                        s.settings.adjust(row, 1);
                                        s.is_dirty = true;
                                        save = true;
                                        if row == FAST_DOWNLOADS_ROW {
                                            limits_changed = true;
                                        }
                                    }
                                }
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

                    if update_ytdlp || update_celestial {
                        let (has_active, busy) = {
                            let s = state.lock().await;
                            (
                                s.downloads.iter().any(|d| {
                                    matches!(
                                        d.status,
                                        DownloadStatus::Loading | DownloadStatus::Downloading(_)
                                    )
                                }),
                                s.notice_busy,
                            )
                        };

                        if busy {
                        } else if has_active {
                            let what = if update_ytdlp {
                                "yt-dlp"
                            } else {
                                "Celestial"
                            };
                            notify(
                                &state,
                                &tx,
                                format!("Stop active downloads before updating {}", what),
                                false,
                                Some(4000),
                            )
                            .await;
                        } else if update_ytdlp {
                            let state_clone = state.clone();
                            let tx_clone = tx.clone();
                            tokio::spawn(async move {
                                notify(
                                    &state_clone,
                                    &tx_clone,
                                    "Updating yt-dlp…",
                                    true,
                                    None,
                                )
                                .await;
                                let msg = downloader::update_yt_dlp().await;
                                notify(
                                    &state_clone,
                                    &tx_clone,
                                    msg,
                                    false,
                                    Some(8000),
                                )
                                .await;
                            });
                        } else {
                            let state_clone = state.clone();
                            let tx_clone = tx.clone();
                            tokio::spawn(async move {
                                notify(
                                    &state_clone,
                                    &tx_clone,
                                    "Checking for updates…",
                                    true,
                                    None,
                                )
                                .await;
                                match updater::check().await {
                                    Ok(Some(release)) => {
                                        let mut s = state_clone.lock().await;
                                        s.notice = None;
                                        s.notice_busy = false;
                                        s.pending_update = Some(release);
                                        s.screen = AppScreen::UpdatePrompt;
                                        s.is_dirty = true;
                                        drop(s);
                                        let _ = tx_clone.send(()).await;
                                    }
                                    Ok(None) => {
                                        let msg = format!(
                                            "Celestial is up to date (v{})",
                                            updater::CURRENT
                                        );
                                        notify(
                                            &state_clone,
                                            &tx_clone,
                                            msg,
                                            false,
                                            Some(6000),
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        notify(
                                            &state_clone,
                                            &tx_clone,
                                            format!("Update check failed: {}", e),
                                            false,
                                            Some(8000),
                                        )
                                        .await;
                                    }
                                }
                            });
                        }
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
                        s.mode = if s.mode == Mode::Audio {
                            Mode::Video
                        } else {
                            Mode::Audio
                        };
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
                    KeyCode::F(4) => {
                        let mut s = state.lock().await;
                        s.screen = AppScreen::History;
                        s.history_cursor = 0;
                        s.is_dirty = true;
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
                            // -STA is required for AutoUpgradeEnabled to show the modern Windows Explorer picker.
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
                            state_guard.input_buffer.is_empty()
                                && state_guard.downloads.iter().any(|d| {
                                    matches!(
                                        d.status,
                                        DownloadStatus::Loading | DownloadStatus::Downloading(_)
                                    )
                                })
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
                                notify(&state, &tx, "Clipboard has no text", false, Some(3000))
                                    .await;
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
                                notify(
                                    &state,
                                    &tx,
                                    format!("Added {} links from clipboard", n),
                                    false,
                                    Some(4000),
                                )
                                .await;
                            }
                        }
                    }
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            || key.modifiers.contains(KeyModifiers::ALT) =>
                    {
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

    Ok(())
}