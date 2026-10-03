use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::process::Command;
use std::process::Stdio;
use crate::{AppState, AppScreen};

async fn download_file(url: &str, dest: &str, state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    
    let client = reqwest::Client::builder().build()?;
    let res = client.get(url).send().await?;
    let total_size = res.content_length().unwrap_or(0);
    
    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded: u64 = 0;
    let mut stream = res.bytes_stream();
    
    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        
        if total_size > 0 {
            let progress = (downloaded as f64 / total_size as f64) * 100.0;
            
            // FIX: We put the lock inside its own scope {} so it drops immediately
            {
                let mut s = state.lock().await;
                s.install_progress = progress as f32;
                s.is_dirty = true;
            } 
            
            // FIX: Use try_send() instead of send().await so it never blocks if the queue is full
            let _ = tx.try_send(());
        }
    }
    Ok(())
}

pub async fn check_and_install(state: Arc<Mutex<AppState>>, tx: mpsc::Sender<()>) {
    let yt_missing = Command::new("yt-dlp").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().await.is_err() && !tokio::fs::metadata("yt-dlp.exe").await.is_ok();
    let ffmpeg_missing = Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().await.is_err() && !tokio::fs::metadata("ffmpeg.exe").await.is_ok();

    if yt_missing || ffmpeg_missing {
        {
            let mut s = state.lock().await;
            s.screen = AppScreen::Installing;
            s.install_progress = 0.0;
            s.is_dirty = true;
        }
        let _ = tx.send(()).await;

        if yt_missing {
            let _ = download_file("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe", "yt-dlp.exe", state.clone(), tx.clone()).await;
        }
        
        if ffmpeg_missing {
            {
                let mut s = state.lock().await;
                s.install_progress = 0.0;
                s.is_dirty = true;
            }
            let _ = tx.send(()).await;
            
            if download_file("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip", "ffmpeg.zip", state.clone(), tx.clone()).await.is_ok() {
                let _ = Command::new("powershell")
                    .args(&["-Command", "Expand-Archive -Path ffmpeg.zip -DestinationPath ffmpeg_extracted -Force"])
                    .status().await;
                
                let _ = Command::new("powershell")
                    .args(&["-Command", "Copy-Item -Path ffmpeg_extracted\\*\\bin\\ffmpeg.exe -Destination .\\ffmpeg.exe -Force; Copy-Item -Path ffmpeg_extracted\\*\\bin\\ffprobe.exe -Destination .\\ffprobe.exe -Force; Remove-Item -Recurse -Force ffmpeg_extracted; Remove-Item -Force ffmpeg.zip"])
                    .status().await;
            }
        }
    }
    
    {
        let mut s = state.lock().await;
        s.screen = AppScreen::Main;
        s.is_dirty = true;
    }
    let _ = tx.send(()).await;
}