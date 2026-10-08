use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

use crate::{notify, AppScreen, AppState};

pub const REPO: &str = "kradengdeng/Celestial";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
}

fn parse_version(v: &str) -> Vec<u64> {
    let core = v
        .trim()
        .trim_start_matches(|c| c == 'v' || c == 'V')
        .split(|c: char| c == '-' || c == '+')
        .next()
        .unwrap_or("");
    core.split('.')
        .map(|p| p.trim().parse().unwrap_or(0))
        .collect()
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    let (mut a, mut b) = (parse_version(remote), parse_version(local));
    let n = a.len().max(b.len()).max(3);
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    s.into()
}

pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(with_suffix(&exe, ".old"));
        let _ = std::fs::remove_file(with_suffix(&exe, ".new"));
    }
}

pub async fn check() -> Result<Option<Release>, String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("Celestial/{}", CURRENT))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    let res = client
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            REPO
        ))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "Could not reach GitHub".to_string())?;

    match res.status().as_u16() {
        404 => return Err("No release published yet".to_string()),
        403 | 429 => return Err("GitHub rate limit reached, try again later".to_string()),
        s if !(200..300).contains(&s) => return Err(format!("GitHub returned HTTP {}", s)),
        _ => {}
    }

    let text = res
        .text()
        .await
        .map_err(|_| "Could not read the GitHub response".to_string())?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "Unexpected GitHub response".to_string())?;

    let tag = json["tag_name"].as_str().ok_or("The release has no tag")?;
    let version = tag.trim_start_matches(|c| c == 'v' || c == 'V').to_string();
    if !is_newer(&version, CURRENT) {
        return Ok(None);
    }

    let assets = json["assets"].as_array().cloned().unwrap_or_default();
    let pick = |prefer_name: bool| {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?.to_lowercase();
            let url = a["browser_download_url"].as_str()?;
            if name.ends_with(".exe") && (!prefer_name || name.contains("celestial")) {
                Some(url.to_string())
            } else {
                None
            }
        })
    };
    let url = pick(true)
        .or_else(|| pick(false))
        .ok_or("The latest release has no .exe file attached")?;

    Ok(Some(Release { version, url }))
}

async fn download_to(
    url: &str,
    path: &Path,
    state: &Arc<Mutex<AppState>>,
    tx: &mpsc::Sender<()>,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(format!("Celestial/{}", CURRENT))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client
        .get(url)
        .send()
        .await
        .map_err(|_| "Download failed".to_string())?;
    if !res.status().is_success() {
        return Err(format!("Download failed (HTTP {})", res.status().as_u16()));
    }

    let total = res.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| format!("Cannot write the update: {}", e))?;
    let mut stream = res.bytes_stream();
    let mut done: u64 = 0;
    let mut last_pct: i32 = -1;

    loop {
        let next = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .map_err(|_| "Download stalled".to_string())?;
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|_| "Download interrupted".to_string())?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Cannot write the update: {}", e))?;
        done += chunk.len() as u64;

        if total > 0 {
            let pct = (done as f64 / total as f64 * 100.0) as i32;
            if pct != last_pct {
                last_pct = pct;
                {
                    let mut s = state.lock().await;
                    s.install_progress = pct as f32;
                    s.is_dirty = true;
                }
                let _ = tx.send(()).await;
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);

    if done < 100_000 || (total > 0 && done != total) {
        return Err("Downloaded file is incomplete".to_string());
    }
    if !looks_like_exe(path).await {
        return Err("Downloaded file is not a valid program".to_string());
    }
    Ok(())
}

async fn looks_like_exe(path: &Path) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return false;
    };
    let mut head = [0u8; 2];
    file.read_exact(&mut head).await.is_ok() && &head == b"MZ"
}

async fn download_and_replace(
    release: &Release,
    state: &Arc<Mutex<AppState>>,
    tx: &mpsc::Sender<()>,
) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|_| "Cannot locate the program file".to_string())?;
    let new_path = with_suffix(&exe, ".new");
    let old_path = with_suffix(&exe, ".old");

    if let Err(e) = download_to(&release.url, &new_path, state, tx).await {
        let _ = tokio::fs::remove_file(&new_path).await;
        return Err(e);
    }

    let _ = tokio::fs::remove_file(&old_path).await;
    if let Err(e) = tokio::fs::rename(&exe, &old_path).await {
        let _ = tokio::fs::remove_file(&new_path).await;
        return Err(format!("Cannot replace the program: {}", e));
    }
    if let Err(e) = tokio::fs::rename(&new_path, &exe).await {
        let _ = tokio::fs::rename(&old_path, &exe).await;
        let _ = tokio::fs::remove_file(&new_path).await;
        return Err(format!("Cannot replace the program: {}", e));
    }
    Ok(exe)
}

pub async fn apply(state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>, release: Release) {
    {
        let mut s = state.lock().await;
        s.pending_update = Some(release.clone());
        s.screen = AppScreen::Updating;
        s.install_progress = 0.0;
        s.loading_text = None;
        s.is_dirty = true;
    }
    let _ = tx.send(()).await;

    match download_and_replace(&release, &state, &tx).await {
        Ok(exe) => {
            {
                let mut s = state.lock().await;
                s.loading_text = Some(format!("Updated to v{}. Restarting…", release.version));
                s.is_dirty = true;
            }
            let _ = tx.send(()).await;
            tokio::time::sleep(Duration::from_millis(1200)).await;

            let exe_str = exe.to_string_lossy().to_string();
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", exe_str.as_str()])
                .spawn();

            let mut s = state.lock().await;
            s.quit = true;
            s.is_dirty = true;
        }
        Err(e) => {
            {
                let mut s = state.lock().await;
                s.screen = AppScreen::Main;
                s.pending_update = None;
                s.loading_text = None;
                s.is_dirty = true;
            }
            let _ = tx.send(()).await;
            notify(
                &state,
                &tx,
                format!("Update failed: {}", e),
                false,
                Some(8000),
            )
            .await;
        }
    }
}

pub async fn finish_loading(state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>) {
    let auto = {
        let mut s = state.lock().await;
        s.loading_text = Some("Checking for updates…".to_string());
        s.is_dirty = true;
        s.settings.auto_update
    };
    let _ = tx.send(()).await;

    let found = match tokio::time::timeout(Duration::from_secs(6), check()).await {
        Ok(Ok(Some(release))) => Some(release),
        _ => None,
    };

    match found {
        Some(release) if auto => {
            apply(state, tx, release).await;
            return;
        }
        Some(release) => {
            let mut s = state.lock().await;
            s.pending_update = Some(release);
            s.screen = AppScreen::UpdatePrompt;
            s.loading_text = None;
            s.is_dirty = true;
        }
        None => {
            let mut s = state.lock().await;
            s.screen = AppScreen::Main;
            s.loading_text = None;
            s.is_dirty = true;
        }
    }
    let _ = tx.send(()).await;
}