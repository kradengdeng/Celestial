use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex, Notify, Semaphore};

use crate::{notify, AppState, DownloadItem, DownloadStatus, Mode};

struct DownloadJob {
    index: usize,
    url: String,
    mode: Mode,
    output_dir: std::path::PathBuf,
}

pub struct DownloadManager {
    state: Arc<Mutex<AppState>>,
    ui_tx: mpsc::Sender<()>,

    /// Pending jobs not yet started (queue mode only).
    /// Shared with the sequential worker so we can drain it on fast-mode toggle.
    pending: Arc<Mutex<VecDeque<DownloadJob>>>,
    worker_notify: Arc<Notify>,
    worker_handle: tokio::task::JoinHandle<()>,

    /// Semaphores that cap fast-mode concurrency.
    audio_sem: Arc<Semaphore>,
    video_sem: Arc<Semaphore>,
    /// Handles of in-flight fast-mode tasks (for cancel).
    fast_tasks: Vec<tokio::task::JoinHandle<()>>,

    /// Playlist expansion runs in background tasks and reports back through this channel.
    expand_tx: mpsc::UnboundedSender<Vec<String>>,
    expand_rx: mpsc::UnboundedReceiver<Vec<String>>,
    expand_tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn yt_dlp_cmd() -> Command {
    if Path::new("yt-dlp.exe").exists() {
        Command::new(".\\yt-dlp.exe")
    } else {
        Command::new("yt-dlp")
    }
}

/// "ERROR: [youtube] abc123: Video unavailable" -> "Video unavailable"
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
    s.to_string()
}

/// Last "ERROR:" line from yt-dlp's stderr, cleaned up.
fn last_error(stderr: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .rev()
        .find(|l| l.starts_with("ERROR:"))
        .map(clean_error)
        .filter(|s| !s.is_empty())
}

/// Returns the playlist id of a URL like `...?v=X&list=PLxxxx`.
/// Auto-generated mixes (`RD...`) are treated as single videos.
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

async fn fetch_playlist(url: &str) -> Result<Vec<String>, String> {
    let output = yt_dlp_cmd()
        .arg("--flat-playlist")
        .arg("--yes-playlist")
        .arg("--print")
        .arg("id")
        .arg(url)
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

/// Runs `yt-dlp -U` and returns a one-line result for the notice row.
pub async fn update_yt_dlp() -> String {
    match yt_dlp_cmd()
        .arg("-U")
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
        audio_slots: usize,
        video_slots: usize,
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
            audio_sem: Arc::new(Semaphore::new(audio_slots.max(1))),
            video_sem: Arc::new(Semaphore::new(video_slots.max(1))),
            fast_tasks: Vec::new(),
            expand_tx,
            expand_rx,
            expand_tasks: Vec::new(),
        }
    }

    // ── Public API ──────────────────────────────────────────────────────────

    pub async fn submit(&mut self, input: String) {
        let input_clean = input.trim().trim_matches('"').to_string();
        if input_clean.to_lowercase().ends_with(".txt") {
            if let Ok(contents) = fs::read_to_string(&input_clean).await {
                for line in contents.lines() {
                    let url = line.trim();
                    if !url.is_empty() {
                        self.add_url(url.to_string()).await;
                    }
                }
                return;
            }
        }
        self.add_url(input_clean).await;
    }

    /// Enqueue videos produced by finished playlist expansions. Call every loop tick.
    pub async fn poll_expanded(&mut self) {
        while let Ok(urls) = self.expand_rx.try_recv() {
            for url in urls {
                self.enqueue(url).await;
            }
        }
    }

    /// Re-read the parallel-download limits from the settings.
    /// Jobs that already acquired a slot keep running under the old limit.
    pub async fn apply_limits(&mut self) {
        let (audio, video) = {
            let s = self.state.lock().await;
            (s.settings.fast_audio_slots, s.settings.fast_video_slots)
        };
        self.audio_sem = Arc::new(Semaphore::new(audio.max(1)));
        self.video_sem = Arc::new(Semaphore::new(video.max(1)));
    }

    /// Called by main when the user presses F6 to toggle fast mode.
    /// If switching TO fast mode, drains all pending Queued jobs and spawns
    /// them in parallel. The currently active (Loading/Downloading) sequential
    /// job is left alone and finishes normally.
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
        // Switching back to queue mode: nothing to do — fast tasks keep running
        // and new submissions will go to the sequential worker.
    }

    pub async fn cancel_all(&mut self) {
        // Stop the sequential worker and clear pending queue.
        self.worker_handle.abort();
        {
            self.pending.lock().await.clear();
        }

        // Abort all in-flight fast tasks and playlist lookups.
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
                    DownloadStatus::Queued | DownloadStatus::Loading | DownloadStatus::Downloading(_)
                ) {
                    item.status = DownloadStatus::Cancelled;
                    changed = true;
                }
            }
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

        // Restart a fresh worker for future queue-mode submissions.
        let (pending, notify) = self.restart_worker();
        self.pending = pending;
        self.worker_notify = notify;
    }

    // ── Private helpers ─────────────────────────────────────────────────────

    async fn add_url(&mut self, url: String) {
        let expand = { self.state.lock().await.settings.expand_playlists };
        if expand && playlist_id(&url).is_some() {
            self.spawn_playlist_fetch(url);
        } else {
            self.enqueue(url).await;
        }
    }

    fn spawn_playlist_fetch(&mut self, url: String) {
        let state = self.state.clone();
        let ui_tx = self.ui_tx.clone();
        let tx = self.expand_tx.clone();
        let handle = tokio::spawn(async move {
            notify(&state, &ui_tx, "Fetching playlist…", true, None).await;
            match fetch_playlist(&url).await {
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
                    notify(&state, &ui_tx, format!("Playlist failed: {}", e), false, Some(6000)).await;
                }
            }
        });
        self.expand_tasks.push(handle);
    }

    async fn enqueue(&mut self, url: String) {
        let fast_mode = { self.state.lock().await.fast_mode };

        let (index, mode, output_dir) = {
            let mut state = self.state.lock().await;
            let index = state.downloads.len();
            let mode = state.mode;
            let output_dir = state.output_path.clone();

            state.downloads.push(DownloadItem {
                url: url.clone(),
                title: None,
                status: if fast_mode { DownloadStatus::Loading } else { DownloadStatus::Queued },
            });
            state.is_dirty = true;

            let (_, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let avail = crate::ui::list_capacity(rows);
            let max_scroll = (index + 1).saturating_sub(avail);
            if state.scroll_offset < max_scroll {
                state.scroll_offset = max_scroll;
            }

            (index, mode, output_dir)
        };
        let _ = self.ui_tx.send(()).await;

        let job = DownloadJob { index, url, mode, output_dir };

        if fast_mode {
            self.spawn_fast_job(job);
        } else {
            self.pending.lock().await.push_back(job);
            self.worker_notify.notify_one();
        }
    }

    fn spawn_fast_job(&mut self, job: DownloadJob) {
        let sem = match job.mode {
            Mode::Audio => self.audio_sem.clone(),
            Mode::Video => self.video_sem.clone(),
        };
        let state_clone = self.state.clone();
        let ui_tx_clone = self.ui_tx.clone();
        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await;
            process_job(job, &state_clone, &ui_tx_clone).await;
        });
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

// ── Sequential worker ────────────────────────────────────────────────────────

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
            // Immediately loop to check for more without waiting.
        } else {
            // Queue is empty — sleep until notified.
            notify.notified().await;
        }
    }
}

// ── Core download logic (shared between queue and fast paths) ─────────────────

async fn process_job(job: DownloadJob, state: &Arc<Mutex<AppState>>, ui_tx: &mpsc::Sender<()>) {
    let DownloadJob { index, url, mode, output_dir } = job;

    // Snapshot the settings so a change mid-download doesn't affect this job.
    let settings = { state.lock().await.settings.clone() };

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
    cmd.arg("--no-playlist")
        .arg("--print")
        .arg("title")
        .arg(&url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut early_error: Option<String> = None;
    let title = match cmd.output().await {
        Ok(output) => {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let first = text.lines().next().unwrap_or("").trim().to_string();
                if first.is_empty() { "Unknown Title".to_string() } else { first }
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

    {
        let s = state.lock().await;
        if s.downloads.get(index).map_or(false, |i| matches!(i.status, DownloadStatus::Cancelled)) {
            return;
        }
    }

    // yt-dlp already told us why this link can't be downloaded: fail now with that reason.
    if let Some(reason) = early_error {
        let mut s = state.lock().await;
        if let Some(item) = s.downloads.get_mut(index) {
            item.status = DownloadStatus::Error(reason);
            s.is_dirty = true;
        }
        drop(s);
        let _ = ui_tx.send(()).await;
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
    dl_cmd.arg("--no-playlist");
    if mode == Mode::Audio {
        dl_cmd.arg("-x").arg("--audio-format").arg(&settings.audio_format);
        if matches!(settings.audio_format.as_str(), "mp3" | "m4a" | "opus") {
            let quality = if settings.audio_quality == "Best" { "0" } else { settings.audio_quality.as_str() };
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
    dl_cmd.arg("-o").arg(&out_tmpl).arg("--newline").arg(&url);
    if Path::new("ffmpeg.exe").exists() {
        dl_cmd.arg("--ffmpeg-location").arg(".\\ffmpeg.exe");
    }
    dl_cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let final_status = if let Ok(mut child) = dl_cmd.spawn() {
        // Collect stderr in the background so the pipe never fills up.
        let err_task = child.stderr.take().map(|stderr| {
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, BufReader};
                let mut lines = BufReader::new(stderr).lines();
                let mut last: Option<String> = None;
                while let Ok(Some(l)) = lines.next_line().await {
                    if l.starts_with("ERROR:") {
                        last = Some(clean_error(&l));
                    }
                }
                last
            })
        });

        if let Some(stdout) = child.stdout.take() {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while let Ok(bytes) = reader.read_line(&mut line).await {
                if bytes == 0 {
                    break;
                }
                if line.contains("[download]") && line.contains('%') {
                    if let Some(start) = line.find("[download]") {
                        let part = &line[start + 10..];
                        if let Some(end) = part.find('%') {
                            if let Ok(p) = part[..end].trim().parse::<f32>() {
                                let mut s = state.lock().await;
                                if let Some(item) = s.downloads.get_mut(index) {
                                    if !matches!(item.status, DownloadStatus::Cancelled) {
                                        item.status = DownloadStatus::Downloading(p);
                                        s.is_dirty = true;
                                    }
                                }
                                drop(s);
                                let _ = ui_tx.send(()).await;
                            }
                        }
                    }
                }
                line.clear();
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
                if s.downloads.get(index).map_or(false, |i| matches!(i.status, DownloadStatus::Cancelled)) {
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

    {
        let mut s = state.lock().await;
        if let Some(item) = s.downloads.get_mut(index) {
            if !matches!(item.status, DownloadStatus::Cancelled) {
                item.status = final_status;
                s.is_dirty = true;
            }
        }
    }
    let _ = ui_tx.send(()).await;
}