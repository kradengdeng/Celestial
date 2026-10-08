use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

use crate::{notify, AppScreen, AppState};

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RETRIES: usize = 3;

async fn set_loading(state: &Arc<Mutex<AppState>>, tx: &mpsc::Sender<()>, text: impl Into<String>) {
    {
        let mut s = state.lock().await;
        s.loading_text = Some(text.into());
        s.is_dirty = true;
    }

    let _ = tx.send(()).await;
}

async fn download_file(
    url: &str,
    dest: &str,
    state: Arc<Mutex<AppState>>,
    tx: mpsc::Sender<()>,
) -> Result<(), String> {
    use futures_util::StreamExt;

    for attempt in 1..=MAX_RETRIES {
        if attempt > 1 {
            set_loading(
                &state,
                &tx,
                format!("Retrying download... ({}/{})", attempt, MAX_RETRIES),
            )
            .await;

            let _ = tokio::fs::remove_file(dest).await;
        }

        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

        let response = match client.get(url).send().await {
            Ok(response) => response,
            Err(e) => {
                if attempt == MAX_RETRIES {
                    return Err(format!("Download connection failed: {}", e));
                }

                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();

            if attempt == MAX_RETRIES {
                return Err(format!("Download failed (HTTP {})", status));
            }

            continue;
        }

        let total_size = response.content_length().unwrap_or(0);

        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| format!("Cannot create {}: {}", dest, e))?;

        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        loop {
            let next_chunk = tokio::time::timeout(DOWNLOAD_TIMEOUT, stream.next()).await;

            let item = match next_chunk {
                Ok(item) => item,
                Err(_) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(dest).await;

                    if attempt == MAX_RETRIES {
                        return Err("Download stalled: no data received for 30 seconds".to_string());
                    }

                    break;
                }
            };

            let Some(item) = item else {
                file.flush()
                    .await
                    .map_err(|e| format!("Failed to finish {}: {}", dest, e))?;

                drop(file);

                if total_size > 0 && downloaded != total_size {
                    if attempt == MAX_RETRIES {
                        return Err(format!(
                            "Download incomplete: received {} of {} bytes",
                            downloaded, total_size
                        ));
                    }

                    let _ = tokio::fs::remove_file(dest).await;
                    break;
                }

                if downloaded == 0 {
                    if attempt == MAX_RETRIES {
                        return Err("Download returned an empty file".to_string());
                    }

                    let _ = tokio::fs::remove_file(dest).await;
                    break;
                }

                return Ok(());
            };

            let chunk = match item {
                Ok(chunk) => chunk,
                Err(e) => {
                    drop(file);
                    let _ = tokio::fs::remove_file(dest).await;

                    if attempt == MAX_RETRIES {
                        return Err(format!("Download interrupted: {}", e));
                    }

                    break;
                }
            };

            file.write_all(&chunk)
                .await
                .map_err(|e| format!("Failed to write {}: {}", dest, e))?;

            downloaded += chunk.len() as u64;

            if total_size > 0 {
                let progress = (downloaded as f64 / total_size as f64 * 100.0) as f32;

                let name = match dest {
                    "yt-dlp.exe" => "yt-dlp",
                    "deno.zip" => "Deno",
                    _ => "FFmpeg",
                };

                {
                    let mut s = state.lock().await;

                    s.install_progress = progress;
                    s.loading_text = Some(format!(
                        "Downloading {}... ({:.1}% Completed)",
                        name, progress
                    ));
                    s.is_dirty = true;
                }

                let _ = tx.send(()).await;
            }
        }
    }

    Err("Download failed after multiple attempts".to_string())
}

async fn installation_failed(state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>, error: String) {
    {
        let mut s = state.lock().await;

        s.screen = AppScreen::Main;
        s.loading_text = None;
        s.install_progress = 0.0;
        s.is_dirty = true;
    }

    let _ = tx.send(()).await;

    notify(
        &state,
        &tx,
        format!("Installation failed: {}", error),
        false,
        Some(10000),
    )
    .await;
}

pub async fn check_and_install(state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>) {
    let yt_missing = Command::new("yt-dlp")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_err()
        && !tokio::fs::metadata("yt-dlp.exe").await.is_ok();

    let ffmpeg_missing = Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_err()
        && !tokio::fs::metadata("ffmpeg.exe").await.is_ok();

    let deno_missing = Command::new("deno")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_err()
        && !tokio::fs::metadata("deno.exe").await.is_ok();

    if !yt_missing && !ffmpeg_missing && !deno_missing {
        crate::updater::finish_loading(state, tx).await;
        return;
    }

    {
        let mut s = state.lock().await;

        s.screen = AppScreen::Installing;
        s.install_progress = 0.0;
        s.loading_text = Some("Installing requirements...".to_string());
        s.is_dirty = true;
    }

    let _ = tx.send(()).await;

    if yt_missing {
        set_loading(&state, &tx, "Downloading yt-dlp...").await;

        if let Err(e) = download_file(
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
            "yt-dlp.exe",
            state.clone(),
            tx.clone(),
        )
        .await
        {
            installation_failed(state, tx, e).await;
            return;
        }

        if tokio::fs::metadata("yt-dlp.exe").await.is_err() {
            installation_failed(
                state,
                tx,
                "yt-dlp.exe was not installed correctly".to_string(),
            )
            .await;

            return;
        }
    }

    if ffmpeg_missing {
        {
            let mut s = state.lock().await;

            s.install_progress = 0.0;
            s.loading_text = Some("Downloading FFmpeg...".to_string());
            s.is_dirty = true;
        }

        let _ = tx.send(()).await;

        if let Err(e) = download_file(
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
            "ffmpeg.zip",
            state.clone(),
            tx.clone(),
        )
        .await
        {
            installation_failed(state, tx, e).await;
            return;
        }

        set_loading(&state, &tx, "Extracting FFmpeg...").await;

        let extract_status = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Expand-Archive -Path ffmpeg.zip -DestinationPath ffmpeg_extracted -Force",
            ])
            .status()
            .await;

        match extract_status {
            Ok(status) if status.success() => {}

            Ok(status) => {
                installation_failed(
                    state,
                    tx,
                    format!("FFmpeg extraction failed (exit code {:?})", status.code()),
                )
                .await;

                return;
            }

            Err(e) => {
                installation_failed(state, tx, format!("Could not start PowerShell: {}", e)).await;

                return;
            }
        }

        set_loading(&state, &tx, "Installing FFmpeg...").await;

        let copy_script = r#"
            $dir = Get-ChildItem -Path "ffmpeg_extracted" -Directory | Select-Object -First 1
            if (-not $dir) { throw "FFmpeg archive folder not found" }
            Copy-Item "$($dir.FullName)\bin\ffmpeg.exe" ".\ffmpeg.exe" -Force
            Copy-Item "$($dir.FullName)\bin\ffprobe.exe" ".\ffprobe.exe" -Force
            Remove-Item -Recurse -Force "ffmpeg_extracted"
            Remove-Item -Force "ffmpeg.zip"
        "#;

        let copy_status = Command::new("powershell")
            .args(["-NoProfile", "-Command", copy_script])
            .status()
            .await;

        match copy_status {
            Ok(status) if status.success() => {}

            Ok(status) => {
                installation_failed(
                    state,
                    tx,
                    format!("FFmpeg installation failed (exit code {:?})", status.code()),
                )
                .await;

                return;
            }

            Err(e) => {
                installation_failed(state, tx, format!("Could not install FFmpeg: {}", e)).await;

                return;
            }
        }

        if tokio::fs::metadata("ffmpeg.exe").await.is_err() {
            installation_failed(
                state,
                tx,
                "ffmpeg.exe was not found after installation".to_string(),
            )
            .await;

            return;
        }

        if tokio::fs::metadata("ffprobe.exe").await.is_err() {
            installation_failed(
                state,
                tx,
                "ffprobe.exe was not found after installation".to_string(),
            )
            .await;

            return;
        }
    }

    if deno_missing {
        {
            let mut s = state.lock().await;

            s.install_progress = 0.0;
            s.loading_text = Some("Downloading Deno...".to_string());
            s.is_dirty = true;
        }

        let _ = tx.send(()).await;

        let downloaded = download_file(
            "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip",
            "deno.zip",
            state.clone(),
            tx.clone(),
        )
        .await
        .is_ok();

        if downloaded {
            set_loading(&state, &tx, "Installing Deno...").await;

            let _ = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    "Expand-Archive -Path deno.zip -DestinationPath . -Force",
                ])
                .status()
                .await;
        }

        let _ = tokio::fs::remove_file("deno.zip").await;
    }

    {
        let mut s = state.lock().await;

        s.loading_text = Some("Requirements installed successfully".to_string());

        s.install_progress = 100.0;
        s.is_dirty = true;
    }

    let _ = tx.send(()).await;

    tokio::time::sleep(Duration::from_millis(500)).await;

    crate::updater::finish_loading(state, tx).await;
}