<div align="center">

# Celestial

**A fast, good-looking terminal app for downloading YouTube audio and video.**
Built in Rust. Powered by `yt-dlp`. Zero setup.

![Rust](https://img.shields.io/badge/Rust-2021-orange?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows&logoColor=white)
![Tokio](https://img.shields.io/badge/async-Tokio-purple)
![TUI](https://img.shields.io/badge/TUI-Crossterm-blueviolet)
![Status](https://img.shields.io/badge/status-active-brightgreen)

<br>

<!-- Put your screenshot at docs/screenshot.png -->
<img src="docs/screenshot.png" alt="Celestial screenshot" width="760">

<br>

[Download](../../releases/latest) &nbsp;|&nbsp; [Controls](#controls) &nbsp;|&nbsp; [Settings](#settings) &nbsp;|&nbsp; [Build from source](#build-from-source)

</div>

---

## Features

| Feature | Description |
|---|---|
| **Custom TUI** | Hand-rendered terminal interface using `crossterm` with TrueColor RGB. |
| **Auto-install** | Missing `yt-dlp` or `ffmpeg`? Celestial downloads and sets them up on first run. |
| **Audio and video** | MP3, M4A, OPUS, FLAC or WAV audio, and MP4 video with a resolution cap. |
| **Playlists** | Paste a playlist link and every video is added to the list. |
| **Batch downloads** | Enter the path of a `.txt` file with one link per line. |
| **Queue and Fast modes** | Download one by one, or many in parallel with an adjustable limit. |
| **Live progress** | Animated spinner, percentage and title for every item. |
| **Clear errors** | Failed items show the real reason, such as "Video unavailable". |
| **Summary line** | Live counts of active, queued, done, failed and cancelled items. |
| **Settings menu** | Change quality, formats, limits and the accent color inside the app. Saved automatically. |
| **Clipboard paste** | `Ctrl+V` pastes one link, or queues several links at once. |
| **yt-dlp updater** | Press `F8` to update `yt-dlp` without leaving the app. |
| **Folder picker** | Press `Tab` to choose the output folder in Windows Explorer. |
| **Instant cancel** | Stop everything with `Space`. |

---

## Requirements

- **OS:** Windows 10 or 11
- **Terminal:** [Windows Terminal](https://aka.ms/terminal) (recommended, for TrueColor and Braille spinner support)
- **Internet connection** for the first-run download of `yt-dlp` and `ffmpeg`

---

## Installation

### Option 1: Release (recommended)

1. Download `Celestial.exe` from the [latest release](../../releases/latest).
2. Put it in its own folder (it stores `yt-dlp.exe`, `ffmpeg.exe` and `settings.json` next to itself).
3. Run it. If Windows SmartScreen warns about an unsigned app, click **More info**, then **Run anyway**.

### Option 2: One-click script

Run `install-and-run.bat`. It installs Rust if needed, builds the project, and starts it.

### Build from source

Requires the [Rust toolchain](https://www.rust-lang.org/tools/install).

```powershell
git clone https://github.com/kradengdeng/Celestial.git
cd Celestial
cargo run --release
```

---

## Controls

### Main screen

| Key | Action |
|:---:|---|
| `Enter` | Confirm the link or file path in the prompt |
| `Ctrl+V` | Paste from the clipboard (several lines add several downloads) |
| `F5` | Switch between audio and video mode |
| `F6` | Toggle Fast DL (parallel downloads) |
| `F7` | Open the settings menu |
| `F8` | Update `yt-dlp` |
| `Tab` | Choose the output folder |
| `Space` | Cancel all active downloads |
| `Up` / `Down` | Scroll the download list |
| `Esc` | Quit |

### Settings screen

| Key | Action |
|:---:|---|
| `Up` / `Down` or `W` / `S` | Select a setting |
| `Left` / `Right` or `A` / `D` | Change the selected value |
| `F7` or `Esc` | Back to the main screen |

---

## Settings

| Setting | Options | Default |
|---|---|---|
| Audio format | MP3, M4A, OPUS, FLAC, WAV | MP3 |
| Audio quality | Best, 320K, 256K, 192K, 128K (lossy formats only) | Best |
| Video quality | Best, 1080p, 720p, 480p, 360p (maximum) | Best |
| Cover art & tags | Off, On (embeds thumbnail and metadata) | Off |
| Playlist links | Expand all, Single video | Expand all |
| Parallel audio | 1, 2, 3, 5, 8, 10, 15, 20, 30 | 30 |
| Parallel video | 1, 2, 3, 5, 8, 10, 15, 20, 30 | 15 |
| Accent color | Light Purple, Blue, Cyan, Green, Yellow, Orange, Pink, Red | Light Purple |

Settings, the selected mode, Fast DL state and output folder are saved to `settings.json` next to the program. Delete the file to reset everything.

---

## Usage

### Single download

Paste a YouTube link at the `>>` prompt and press `Enter`.

```text
>> https://www.youtube.com/watch?v=XXXXXXXXXXX
```

### Playlist

Paste a playlist link. Every video is added as its own row.

```text
>> https://www.youtube.com/playlist?list=PLxxxxxxxxxxxx
```

Set **Playlist links** to *Single video* in the settings to download only the video in the link.

### Batch download

Create a `.txt` file with one link per line, then enter its full path:

```text
>> C:\Users\User\Downloads\list.txt
```

### Queue vs. Fast mode

| Mode | Behavior | Best for |
|---|---|---|
| **Queue** (`Fast: OFF`) | One download at a time, in order | Stable, predictable runs |
| **Fast** (`Fast: ON`) | Many downloads at once, up to the parallel limit | Speed on large lists |

### Status indicators

| Status | Meaning |
|---|---|
| `Queued` | Waiting for its turn |
| `⠋ Loading…` | Fetching video information |
| `⠹ Downloading…` | In progress, with live percentage |
| `✓ Download complete` | Finished and saved |
| `✗ Download failed` | Failed, with the reason from `yt-dlp` |
| `CANCELLED` | Stopped by the user |

---

## Project structure

```text
src/
  main.rs         App state, input handling, main loop
  ui.rs           Terminal rendering (main screen, settings, spinner)
  downloader.rs   Queue, parallel jobs, playlists, yt-dlp calls
  installer.rs    First-run download of yt-dlp and ffmpeg
  settings.rs     Settings model and settings.json storage
```

---

## Built with

- [Rust](https://www.rust-lang.org/)
- [Tokio](https://tokio.rs/): async runtime
- [Crossterm](https://github.com/crossterm-rs/crossterm): terminal control
- [Reqwest](https://github.com/seanmonstar/reqwest): dependency downloads
- [Serde](https://serde.rs/): settings storage
- [arboard](https://github.com/1Password/arboard): clipboard access
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [FFmpeg](https://ffmpeg.org/): download and conversion

---

## Contributing

Issues and pull requests are welcome. [Open an issue](../../issues) for bugs or ideas.

---

## Disclaimer

This project is for **educational purposes only**. Respect YouTube's Terms of Service and the rights of content creators. Only download content you have permission to save.
