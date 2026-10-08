use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{ChildStderr, Command};
use tokio::sync::{mpsc, Mutex, Notify, Semaphore};

use crate::{history, notify, AppState, DownloadItem, DownloadStatus, Mode};

struct DownloadJob {
    index: usize,
    url: String,
    mode: Mode,
    output_dir: std::path::PathBuf,
}

pub struct DownloadManager {
    state: Arc<Mutex<AppState>>,
    ui_tx: mpsc::Sender<()>,

    pending: Arc<Mutex<VecDeque<DownloadJob>>>,
    worker_notify: Arc<Notify>,
    worker_handle: tokio::task::JoinHandle<()>,

    fast_sem: Arc<Semaphore>,
    fast_limit: usize,
    fast_tasks: Vec<tokio::task::JoinHandle<()>>,

    expand_tx: mpsc::UnboundedSender<Vec<String>>,
    expand_rx: mpsc::UnboundedReceiver<Vec<String>>,
    expand_tasks: Vec<tokio::task::JoinHandle<()>>,
}

// v1.2.0 command builder (no deno runtime flag).
fn yt_dlp_cmd() -> Command {
    let mut cmd = if Path::new("yt-dlp.exe").exists() {
        Command::new(".\\yt-dlp.exe")
    } else {
        Command::new("yt-dlp")
    };
    // Make yt-dlp print UTF-8 so titles and error messages are not garbled on Windows.
    cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    cmd
}

fn cookie_args(source: &str) -> Result<Vec<String>, String> {
    match source {
        "Off" => Ok(Vec::new()),
        "cookies.txt" => {
            if Path::new("cookies.txt").exists() {
                Ok(vec!["--cookies".to_string(), "cookies.txt".to_string()])
            } else {
                Err("cookies.txt not found next to the program".to_string())
            }
        }
        browser => Ok(vec![
            "--cookies-from-browser".to_string(),
            browser.to_lowercase(),
        ]),
    }
}

fn clean_error(line: &str) -> String {
    let mut s: &str = line.trim().trim_start_matches("ERROR:").trim();
    if s.starts_with('[') {
        if let Some(end) = s.find(']') {
            s = s[end + 1..].trim();
            if let Some(colon) = s.find(": ") {
                if !s[..colon].contains(' ') {
                    s = &s[colon + 2..];
                }
            }
        }
    }
    let lower = s.to_lowercase();
    if lower.contains("not a bot") {
        return "Bot check: set Cookies in Settings or update yt-dlp".to_string();
    }
    if lower.contains("confirm your age") {
        return "Age-restricted: set Cookies in Settings".to_string();
    }
    if lower.contains("could not copy") && lower.contains("cookie") {
        return "Browser cookies locked: close the browser, or use Firefox / cookies.txt"
            .to_string();
    }
    if lower.contains("dpapi") {
        return "Browser cookies unreadable: try Firefox or cookies.txt".to_string();
    }
    s.to_string()
}

fn last_error(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .map(clean_error)
        .filter(|s| !s.is_empty())
}

async fn read_stderr_error(stderr: ChildStderr) -> Option<String> {
    let mut reader = BufReader::new(stderr);
    let mut buf = Vec::new();
    let mut last = None;
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end();
        if line.starts_with("ERROR:") {
            last = Some(clean_error(line));
        }
    }
    last
}

fn parse_progress(line: &str) -> Option<f32> {
    let start = line.find("[download]")? + "[download]".len();
    let part = &line[start..];
    let end = part.find('%')?;
    part[..end].trim().parse::<f32>().ok()
}

#[cfg(windows)]
fn play_notify_sound() {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBeep(kind: u32) -> i32;
    }
    unsafe {
        MessageBeep(0x40);
    }
}

#[cfg(not(windows))]
fn play_notify_sound() {}

fn take_batch_sound(state: &mut AppState) -> bool {
    let active = state.downloads.iter().any(|d| {
        matches!(
            d.status,
            DownloadStatus::Queued | DownloadStatus::Loading | DownloadStatus::Downloading(_)
        )
    });
    if active || state.batch_completed == 0 {
        return false;
    }
    state.batch_completed = 0;
    state.settings.notify_sound
}

fn is_cancelled(state: &AppState, index: usize) -> bool {
    state
        .downloads
        .get(index)
        .map_or(false, |i| matches!(i.status, DownloadStatus::Cancelled))
}

fn record_history(state: &mut AppState, index: usize, mode: Mode, status: &str) {
    let Some(item) = state.downloads.get(index) else {
        return;
    };
    let url = item.url.clone();
    let title = item.title.clone().unwrap_or_else(|| url.clone());
    history::record(&mut state.history, &url, &title, mode, status);
}

fn is_duplicate(state: &AppState, url: &str) -> bool {
    if history::contains_url(&state.history, url) {
        return true;
    }
    let id = history::video_id(url);
    state.downloads.iter().any(|item| {
        let same = match (&id, history::video_id(&item.url)) {
            (Some(a), Some(b)) => *a == b,
            _ => item.url == url,
        };
        same && matches!(
            item.status,
            DownloadStatus::Queued
                | DownloadStatus::Loading
                | DownloadStatus::Downloading(_)
                | DownloadStatus::Completed
        )
    })
}

fn playlist_id(url: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("list=") {
            let v = v.split('#').next().unwrap_or("");
            if !v.is_empty() && !v.starts_with("RD") {
                return Some(v.to_string());
            }
        }
    }
    None
}

async fn fetch_playlist(url: &str, cookies: &[String]) -> Result<Vec<String>, String> {
    let output = yt_dlp_cmd()
        .args(cookies)
        .arg("--flat-playlist")
        .arg("--yes-playlist")
        .arg("--print")
        .arg("id")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|_| "Failed to start yt-dlp".to_string())?;

    let ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|id| format!("https://www.youtube.com/watch?v={}", id))
        .collect();

    if ids.is_empty() {
        return Err(last_error(&output.stderr)
            .unwrap_or_else(|| "Playlist is empty or unavailable".to_string()));
    }
    Ok(ids)
}

fn video_format(max_height: u32) -> String {
    if max_height == 0 {
        "bestvideo[ext=mp4]+bestaudio[ext=m4a]/best[ext=mp4]/best".to_string()
    } else {
        let h = max_height;
        format!(
            "bestvideo[height<={h}][ext=mp4]+bestaudio[ext=m4a]/best[height<={h}][ext=mp4]/best[height<={h}]/best"
        )
    }
}

pub async fn update_yt_dlp() -> String {
    match yt_dlp_cmd()
        .arg("-U")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(out) => {
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            match text.lines().map(str::trim).rev().find(|l| !l.is_empty()) {
                Some(l) if l.starts_with("ERROR:") => format!("Update failed: {}", clean_error(l)),
                Some(l) => l.to_string(),
                None => "yt-dlp returned no output".to_string(),
            }
        }
        Err(_) => "Failed to start yt-dlp".to_string(),
    }
}

impl DownloadManager {
    pub fn new(
        state: Arc<Mutex<AppState>>,
        ui_tx: mpsc::Sender<()>,
        fast_slots: usize,
    ) -> Self {
        let pending = Arc::new(Mutex::new(VecDeque::<DownloadJob>::new()));
        let notify = Arc::new(Notify::new());

        let worker_handle = tokio::spawn(run_queue_worker(
            pending.clone(),
            notify.clone(),
            state.clone(),
            ui_tx.clone(),
        ));

        let (expand_tx, expand_rx) = mpsc::unbounded_channel();

        Self {
            state,
            ui_tx,
            pending,
            worker_notify: notify,
            worker_handle,
            fast_sem: Arc::new(Semaphore::new(fast_slots.max(1))),
            fast_limit: fast_slots.max(1),
            fast_tasks: Vec::new(),
            expand_tx,
            expand_rx,
            expand_tasks: Vec::new(),
        }
    }

    pub async fn submit(&mut self, input: String) {
        let input_clean = input.trim().trim_matches('"').to_string();
        if input_clean.to_lowercase().ends_with(".txt") && !input_clean.starts_with("http") {
            match fs::read(&input_clean).await {
                Ok(bytes) => {
                    let contents = String::from_utf8_lossy(&bytes).into_owned();
                    for line in contents.trim_start_matches('\u{feff}').lines() {
                        let url = line.trim();
                        if !url.is_empty() {
                            self.add_url(url.to_string()).await;
                        }
                    }
                }
                Err(e) => {
                    notify(
                        &self.state,
                        &self.ui_tx,
                        format!("Cannot read file: {}", e),
                        false,
                        Some(5000),
                    )
                    .await;
                }
            }
            return;
        }
        self.add_url(input_clean).await;
    }

    pub async fn resubmit(&mut self, url: String) {
        self.enqueue(url, false).await;
    }

    pub async fn poll_expanded(&mut self) {
        while let Ok(urls) = self.expand_rx.try_recv() {
            for url in urls {
                self.enqueue(url, true).await;
            }
        }
    }

    pub async fn apply_limits(&mut self) {
        let target = { self.state.lock().await.settings.fast_downloads.max(1) };
        if target > self.fast_limit {
            self.fast_sem.add_permits(target - self.fast_limit);
        } else if target < self.fast_limit {
            let excess = (self.fast_limit - target) as u32;
            let sem = self.fast_sem.clone();
            tokio::spawn(async move {
                if let Ok(permits) = sem.acquire_many(excess).await {
                    permits.forget();
                }
            });
        }
        self.fast_limit = target;
    }

    pub async fn on_fast_mode_toggled(&mut self, fast: bool) {
        if fast {
            let drained: Vec<DownloadJob> = {
                let mut q = self.pending.lock().await;
                q.drain(..).collect()
            };
            for job in drained {
                self.spawn_fast_job(job);
            }
        }
    }

    pub async fn cancel_all(&mut self) {
        self.worker_handle.abort();
        {
            self.pending.lock().await.clear();
        }

        for t in self.fast_tasks.drain(..) {
            t.abort();
        }
        for t in self.expand_tasks.drain(..) {
            t.abort();
        }
        while self.expand_rx.try_recv().is_ok() {}

        {
            let mut state = self.state.lock().await;
            let mut changed = false;
            for item in state.downloads.iter_mut() {
                if matches!(
                    item.status,
                    DownloadStatus::Queued
                        | DownloadStatus::Loading
                        | DownloadStatus::Downloading(_)
                ) {
                    item.status = DownloadStatus::Cancelled;
                    changed = true;
                }
            }
            state.batch_completed = 0;
            if state.notice_busy {
                state.notice = None;
                state.notice_busy = false;
                changed = true;
            }
            if changed {
                state.is_dirty = true;
            }
        }
        let _ = self.ui_tx.send(()).await;

        let (pending, notify) = self.restart_worker();
        self.pending = pending;
        self.worker_notify = notify;
    }

    async fn add_url(&mut self, url: String) {
        let expand = { self.state.lock().await.settings.expand_playlists };
        if expand && playlist_id(&url).is_some() {
            self.spawn_playlist_fetch(url);
        } else {
            self.enqueue(url, true).await;
        }
    }

    fn spawn_playlist_fetch(&mut self, url: String) {
        let state = self.state.clone();
        let ui_tx = self.ui_tx.clone();
        let tx = self.expand_tx.clone();
        let handle = tokio::spawn(async move {
            notify(&state, &ui_tx, "Fetching playlist…", true, None).await;
            let source = { state.lock().await.settings.cookies.clone() };
            let cookies = match cookie_args(&source) {
                Ok(a) => a,
                Err(e) => {
                    notify(
                        &state,
                        &ui_tx,
                        format!("Playlist failed: {}", e),
                        false,
                        Some(6000),
                    )
                    .await;
                    return;
                }
            };
            match fetch_playlist(&url, &cookies).await {
                Ok(urls) => {
                    let n = urls.len();
                    let _ = tx.send(urls);
                    notify(
                        &state,
                        &ui_tx,
                        format!("Added {} videos from playlist", n),
                        false,
                        Some(4000),
                    )
                    .await;
                }
                Err(e) => {
                    notify(
                        &state,
                        &ui_tx,
                        format!("Playlist failed: {}", e),
                        false,
                        Some(6000),
                    )
                    .await;
                }
            }
        });
        self.expand_tasks.retain(|h| !h.is_finished());
        self.expand_tasks.push(handle);
    }

    async fn enqueue(&mut self, url: String, check_duplicate: bool) {
        let queued = {
            let mut state = self.state.lock().await;
            if check_duplicate && state.settings.duplicate_detection && is_duplicate(&state, &url) {
                None
            } else {
                let index = state.downloads.len();
                let mode = state.mode;
                let output_dir = state.output_path.clone();
                let fast_mode = state.fast_mode;

                state.downloads.push(DownloadItem {
                    url: url.clone(),
                    title: None,
                    status: DownloadStatus::Queued,
                });
                state.is_dirty = true;

                let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                let avail = crate::ui::list_capacity(rows);
                let max_scroll = (index + 1).saturating_sub(avail);
                if state.scroll_offset < max_scroll {
                    state.scroll_offset = max_scroll;
                }

                Some((index, mode, output_dir, fast_mode))
            }
        };

        let Some((index, mode, output_dir, fast_mode)) = queued else {
            notify(
                &self.state,
                &self.ui_tx,
                "Skipped duplicate: already downloaded",
                false,
                Some(4000),
            )
            .await;
            return;
        };
        let _ = self.ui_tx.send(()).await;

        let job = DownloadJob {
            index,
            url,
            mode,
            output_dir,
        };

        if fast_mode {
            self.spawn_fast_job(job);
        } else {
            self.pending.lock().await.push_back(job);
            self.worker_notify.notify_one();
        }
    }

    fn spawn_fast_job(&mut self, job: DownloadJob) {
        let sem = self.fast_sem.clone();
        let state_clone = self.state.clone();
        let ui_tx_clone = self.ui_tx.clone();
        let handle = tokio::spawn(async move {
            let _permit = match sem.acquire().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            process_job(job, &state_clone, &ui_tx_clone).await;
        });
        self.fast_tasks.retain(|h| !h.is_finished());
        self.fast_tasks.push(handle);
    }

    fn restart_worker(&mut self) -> (Arc<Mutex<VecDeque<DownloadJob>>>, Arc<Notify>) {
        let pending = Arc::new(Mutex::new(VecDeque::<DownloadJob>::new()));
        let notify = Arc::new(Notify::new());
        self.worker_handle = tokio::spawn(run_queue_worker(
            pending.clone(),
            notify.clone(),
            self.state.clone(),
            self.ui_tx.clone(),
        ));
        (pending, notify)
    }
}

async fn run_queue_worker(
    pending: Arc<Mutex<VecDeque<DownloadJob>>>,
    notify: Arc<Notify>,
    state: Arc<Mutex<AppState>>,
    ui_tx: mpsc::Sender<()>,
) {
    loop {
        let job = { pending.lock().await.pop_front() };
        if let Some(job) = job {
            process_job(job, &state, &ui_tx).await;
        } else {
            notify.notified().await;
        }
    }
}

// ── Core download logic: v1.2.0 flow (plain title lookup, then download) ──────

async fn process_job(job: DownloadJob, state: &Arc<Mutex<AppState>>, ui_tx: &mpsc::Sender<()>) {
    let DownloadJob {
        index,
        url,
        mode,
        output_dir,
    } = job;

    // Snapshot the settings so a change mid-download doesn't affect this job.
    let settings = { state.lock().await.settings.clone() };

    let cookies = match cookie_args(&settings.cookies) {
        Ok(a) => a,
        Err(reason) => {
            let play = {
                let mut s = state.lock().await;
                if is_cancelled(&s, index) {
                    return;
                }
                if let Some(item) = s.downloads.get_mut(index) {
                    item.status = DownloadStatus::Error(reason);
                    s.is_dirty = true;
                }
                take_batch_sound(&mut s)
            };
            let _ = ui_tx.send(()).await;
            if play {
                play_notify_sound();
            }
            return;
        }
    };

    // Phase 1: mark Loading & fetch title
    {
        let mut s = state.lock().await;
        if let Some(item) = s.downloads.get_mut(index) {
            if matches!(item.status, DownloadStatus::Cancelled) {
                return;
            }
            item.status = DownloadStatus::Loading;
            s.is_dirty = true;
        }
    }
    let _ = ui_tx.send(()).await;

    let mut cmd = yt_dlp_cmd();
    cmd.args(&cookies)
        .arg("--no-playlist")
        .arg("--print")
        .arg("title")
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut early_error: Option<String> = None;
    let title = match cmd.output().await {
        Ok(output) => {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let first = text.lines().next().unwrap_or("").trim().to_string();
                if first.is_empty() {
                    "Unknown Title".to_string()
                } else {
                    first
                }
            } else {
                early_error = last_error(&output.stderr);
                "Unknown Title".to_string()
            }
        }
        Err(_) => {
            early_error = Some("Failed to start yt-dlp".to_string());
            "Unknown Title".to_string()
        }
    };

    if is_cancelled(&*state.lock().await, index) {
        return;
    }

    // yt-dlp already told us why this link can't be downloaded: fail now with that reason.
    if let Some(reason) = early_error {
        let play = {
            let mut s = state.lock().await;
            if let Some(item) = s.downloads.get_mut(index) {
                item.status = DownloadStatus::Error(reason);
                s.is_dirty = true;
            }
            record_history(&mut s, index, mode, "Failed");
            take_batch_sound(&mut s)
        };
        let _ = ui_tx.send(()).await;
        if play {
            play_notify_sound();
        }
        return;
    }

    {
        let mut s = state.lock().await;
        if let Some(item) = s.downloads.get_mut(index) {
            item.title = Some(title.clone());
            item.status = DownloadStatus::Downloading(0.0);
            s.is_dirty = true;
        }
    }
    let _ = ui_tx.send(()).await;

    // Phase 2: actual download
    let out_tmpl = output_dir.join("%(title)s.%(ext)s");
    let mut dl_cmd = yt_dlp_cmd();
    dl_cmd.args(&cookies).arg("--no-playlist");
    if mode == Mode::Audio {
        dl_cmd
            .arg("-x")
            .arg("--audio-format")
            .arg(&settings.audio_format);
        if matches!(settings.audio_format.as_str(), "mp3" | "m4a" | "opus") {
            let quality = if settings.audio_quality == "Best" {
                "0"
            } else {
                settings.audio_quality.as_str()
            };
            dl_cmd.arg("--audio-quality").arg(quality);
        }
    } else {
        dl_cmd.arg("-f").arg(video_format(settings.video_height));
    }
    if settings.embed_meta {
        dl_cmd.arg("--embed-metadata");
        // WAV cannot hold cover art.
        if !(mode == Mode::Audio && settings.audio_format == "wav") {
            dl_cmd.arg("--embed-thumbnail");
        }
    }
    if settings.auto_filename {
        dl_cmd
            .arg("--windows-filenames")
            .arg("--trim-filenames")
            .arg("180");
    }
    dl_cmd.arg("-o").arg(&out_tmpl).arg("--newline").arg(&url);
    if Path::new("ffmpeg.exe").exists() {
        dl_cmd.arg("--ffmpeg-location").arg(".\\ffmpeg.exe");
    }
    dl_cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let final_status = if let Ok(mut child) = dl_cmd.spawn() {
        // Collect stderr in the background so the pipe never fills up.
        let err_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(read_stderr_error(stderr)));

        if let Some(stdout) = child.stdout.take() {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&buf);
                if let Some(p) = parse_progress(&line) {
                    {
                        let mut s = state.lock().await;
                        if let Some(item) = s.downloads.get_mut(index) {
                            if !matches!(item.status, DownloadStatus::Cancelled) {
                                item.status = DownloadStatus::Downloading(p);
                                s.is_dirty = true;
                            }
                        }
                    }
                    let _ = ui_tx.send(()).await;
                }
            }
        }

        let exit = child.wait().await;
        let err_line: Option<String> = match err_task {
            Some(handle) => handle.await.ok().flatten(),
            None => None,
        };

        match exit {
            Ok(exit) if exit.success() => DownloadStatus::Completed,
            Ok(_) => {
                let s = state.lock().await;
                if is_cancelled(&s, index) {
                    DownloadStatus::Cancelled
                } else {
                    DownloadStatus::Error(err_line.unwrap_or_else(|| "Failed".to_string()))
                }
            }
            Err(_) => DownloadStatus::Error("Failed to wait".to_string()),
        }
    } else {
        DownloadStatus::Error("Failed to start yt-dlp".to_string())
    };

    let history_status = match &final_status {
        DownloadStatus::Completed => Some("Completed"),
        DownloadStatus::Error(_) => Some("Failed"),
        _ => None,
    };

    let play = {
        let mut s = state.lock().await;
        if let Some(item) = s.downloads.get_mut(index) {
            if !matches!(item.status, DownloadStatus::Cancelled) {
                item.status = final_status;
                s.is_dirty = true;
            }
        }
        if let Some(status) = history_status {
            if status == "Completed" {
                s.batch_completed += 1;
            }
            record_history(&mut s, index, mode, status);
        }
        take_batch_sound(&mut s)
    };
    let _ = ui_tx.send(()).await;
    if play {
        play_notify_sound();
    }
}