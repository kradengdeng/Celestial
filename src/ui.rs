use crossterm::{
    cursor::{Hide, MoveTo, Show},
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::collections::HashMap;
use std::io::{self, stdout, Stdout, Write};
use std::sync::{Mutex, OnceLock};

use crate::{updater, AppScreen, AppState, DownloadStatus, Mode};

pub const HEADER_LINES: u16 = 6;
pub const FOOTER_LINES: u16 = 2;

const LABEL_W: usize = 23;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn list_capacity(rows: u16) -> usize {
    rows.saturating_sub(HEADER_LINES + FOOTER_LINES) as usize
}

fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    }
}

fn draw_footer(
    out: &mut Stdout,
    row: u16,
    cols: u16,
    accent: Color,
    items: &[(&str, &str)],
) -> io::Result<()> {
    out.queue(MoveTo(0, row))?;

    let mut used = 0usize;

    for (key, label) in items {
        let need = key.chars().count() + 1 + label.chars().count() + 2;

        if used + need > cols as usize {
            break;
        }

        out.queue(SetForegroundColor(accent))?;
        out.queue(Print(*key))?;

        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print(format!(" {}  ", label)))?;

        used += need;
    }

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    Ok(())
}

pub fn draw(state: &AppState) -> io::Result<()> {
    let mut out = stdout();

    out.queue(Hide)?;

    let (cols, rows) = crossterm::terminal::size()?;
    let spin = SPINNER[state.spinner_frame % SPINNER.len()];

    match state.screen {
        AppScreen::Installing => draw_installing(&mut out, state, rows, spin),
        AppScreen::Settings => draw_settings(&mut out, state, cols, rows),
        AppScreen::History => draw_history(&mut out, state, cols, rows),
        AppScreen::UpdatePrompt => draw_update_prompt(&mut out, state, rows),
        AppScreen::Updating => draw_updating(&mut out, state, rows, spin),
        AppScreen::Main => draw_main(&mut out, state, cols, rows, spin),
    }
}

fn draw_loading_screen(
    out: &mut Stdout,
    state: &AppState,
    rows: u16,
    spin: char,
    default_text: String,
) -> io::Result<()> {
    let accent = state.settings.accent_color();

    let text = match &state.loading_text {
        Some(text) => text.as_str(),
        None => default_text.as_str(),
    };

    out.queue(MoveTo(0, 0))?;

    out.queue(SetForegroundColor(accent))?;
    out.queue(Print(spin))?;

    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print(format!(" {}", text)))?;

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    for row in 1..rows {
        out.queue(MoveTo(0, row))?;
        out.queue(Clear(ClearType::CurrentLine))?;
    }

    out.queue(Hide)?;
    out.flush()?;

    Ok(())
}

fn draw_installing(out: &mut Stdout, state: &AppState, rows: u16, spin: char) -> io::Result<()> {
    draw_loading_screen(
        out,
        state,
        rows,
        spin,
        format!(
            "Installing requirements... ({:.1}% Completed)",
            state.install_progress
        ),
    )
}

fn draw_updating(out: &mut Stdout, state: &AppState, rows: u16, spin: char) -> io::Result<()> {
    let version = state
        .pending_update
        .as_ref()
        .map(|r| r.version.as_str())
        .unwrap_or("");

    draw_loading_screen(
        out,
        state,
        rows,
        spin,
        format!(
            "Updating Celestial to v{}... ({:.1}%)",
            version, state.install_progress
        ),
    )
}

fn draw_update_prompt(out: &mut Stdout, state: &AppState, rows: u16) -> io::Result<()> {
    let accent = state.settings.accent_color();

    let version = state
        .pending_update
        .as_ref()
        .map(|r| r.version.as_str())
        .unwrap_or("");

    out.queue(MoveTo(0, 0))?;
    out.queue(SetForegroundColor(accent))?;
    out.queue(Print("Update available"))?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 2))?;
    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print(format!(
        "Celestial v{} is available (you have v{}).",
        version,
        updater::CURRENT
    )))?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 3))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print("Your settings are kept. Updating is optional."))?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 5))?;
    out.queue(SetForegroundColor(accent))?;
    out.queue(Print("U"))?;

    out.queue(SetForegroundColor(Color::Grey))?;
    out.queue(Print(" Update now    "))?;

    out.queue(SetForegroundColor(accent))?;
    out.queue(Print("Enter"))?;

    out.queue(SetForegroundColor(Color::Grey))?;
    out.queue(Print(" Continue"))?;

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    for r in [1u16, 4] {
        out.queue(MoveTo(0, r))?;
        out.queue(Clear(ClearType::CurrentLine))?;
    }

    for r in 6..rows {
        out.queue(MoveTo(0, r))?;
        out.queue(Clear(ClearType::CurrentLine))?;
    }

    out.queue(Hide)?;
    out.flush()?;

    Ok(())
}

fn draw_settings(out: &mut Stdout, state: &AppState, cols: u16, rows: u16) -> io::Result<()> {
    let accent = state.settings.accent_color();

    out.queue(MoveTo(0, 0))?;
    out.queue(SetForegroundColor(accent))?;
    out.queue(Print("Settings"))?;

    out.queue(SetForegroundColor(Color::DarkGrey))?;
    out.queue(Print("   changes are saved automatically"))?;

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    draw_notice(out, state, cols, 1)?;

    let hint_w = (cols as usize).saturating_sub(2 + 22 + 18);

    for (i, (label, value, hint)) in state.settings.rows().iter().enumerate() {
        let selected = i == state.settings_cursor;

        out.queue(MoveTo(0, 2 + i as u16))?;

        if selected {
            out.queue(SetForegroundColor(accent))?;
            out.queue(Print("> "))?;

            out.queue(SetForegroundColor(Color::White))?;
        } else {
            out.queue(ResetColor)?;
            out.queue(Print("  "))?;

            out.queue(SetForegroundColor(Color::Grey))?;
        }

        out.queue(Print(format!("{:<22}", label)))?;

        let shown = if selected {
            format!("‹ {} ›", value)
        } else {
            format!("  {}  ", value)
        };

        out.queue(SetForegroundColor(if selected {
            accent
        } else {
            Color::White
        }))?;

        out.queue(Print(format!("{:<18}", shown)))?;

        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print(fit(hint, hint_w)))?;

        out.queue(ResetColor)?;
        out.queue(Clear(ClearType::UntilNewLine))?;
    }

    let footer_row = rows.saturating_sub(2);

    let row_count = state.settings.rows().len() as u16;

    for r in (2 + row_count)..footer_row {
        out.queue(MoveTo(0, r))?;
        out.queue(Clear(ClearType::CurrentLine))?;
    }

    let credit_row = 3 + row_count;

    if credit_row < footer_row {
        out.queue(MoveTo(2, credit_row))?;

        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print(format!(
            "Celestial v{}  |  Made by ",
            updater::CURRENT
        )))?;

        out.queue(SetForegroundColor(accent))?;
        out.queue(Print("@kradengdeng"))?;

        out.queue(ResetColor)?;
    }

    draw_footer(
        out,
        footer_row,
        cols,
        accent,
        &[
            ("↑/↓ W/S", "Select"),
            ("←/→ A/D", "Change"),
            ("F7/Esc", "Back"),
        ],
    )?;

    out.queue(MoveTo(0, rows.saturating_sub(1)))?;
    out.queue(Clear(ClearType::CurrentLine))?;

    out.queue(Hide)?;
    out.flush()?;

    Ok(())
}

fn draw_summary(out: &mut Stdout, state: &AppState) -> io::Result<()> {
    let accent = state.settings.accent_color();

    let (mut active, mut queued, mut done, mut failed, mut cancelled) = (0, 0, 0, 0, 0);

    for d in &state.downloads {
        match d.status {
            DownloadStatus::Queued => queued += 1,
            DownloadStatus::Loading | DownloadStatus::Downloading(_) => active += 1,
            DownloadStatus::Completed => done += 1,
            DownloadStatus::Error(_) => failed += 1,
            DownloadStatus::Cancelled => cancelled += 1,
        }
    }

    out.queue(MoveTo(0, 4))?;

    let parts: [(&str, usize, Color); 5] = [
        ("Active", active, accent),
        ("Queued", queued, Color::Grey),
        ("Done", done, Color::Green),
        ("Failed", failed, Color::Red),
        ("Cancelled", cancelled, Color::DarkGrey),
    ];

    if !state.downloads.is_empty() {
        for (label, count, color) in parts {
            if count > 0 {
                out.queue(SetForegroundColor(color))?;
                out.queue(Print(format!("{} {}", label, count)))?;

                out.queue(SetForegroundColor(Color::DarkGrey))?;
                out.queue(Print("  "))?;
            }
        }

        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print(format!("({} total)", state.downloads.len())))?;
    }

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    Ok(())
}

fn draw_notice(out: &mut Stdout, state: &AppState, cols: u16, row: u16) -> io::Result<()> {
    out.queue(MoveTo(0, row))?;

    if let Some(text) = &state.notice {
        if state.notice_busy {
            let spin = SPINNER[state.spinner_frame % SPINNER.len()];
            out.queue(SetForegroundColor(state.settings.accent_color()))?;
            out.queue(Print(format!("{} ", spin)))?;

            out.queue(SetForegroundColor(Color::Grey))?;
            out.queue(Print(fit(text, (cols as usize).saturating_sub(2))))?;
        } else {
            out.queue(SetForegroundColor(Color::Grey))?;
            out.queue(Print(fit(text, cols as usize)))?;
        }

        out.queue(ResetColor)?;
    }

    out.queue(Clear(ClearType::UntilNewLine))?;
    Ok(())
}

static TIME_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn time_cache() -> &'static Mutex<HashMap<String, String>> {
    TIME_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn prime_history_times(values: &[&str]) {
    let mut missing: Vec<(&str, i64)> = Vec::new();
    {
        let cache = time_cache().lock().unwrap();
        for v in values {
            if cache.contains_key(*v) || missing.iter().any(|(m, _)| m == v) {
                continue;
            }
            if let Ok(seconds) = v.trim().parse::<i64>() {
                missing.push((v, seconds));
            }
        }
    }
    if missing.is_empty() {
        return;
    }

    let list = missing
        .iter()
        .map(|(_, seconds)| seconds.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "@({}) | ForEach-Object {{ [DateTimeOffset]::FromUnixTimeSeconds($_).ToLocalTime().ToString('hh:mm tt, dd MMM yyyy', [Globalization.CultureInfo]::InvariantCulture) }}",
        list
    );

    let formatted: Vec<String> = match std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };

    let mut cache = time_cache().lock().unwrap();
    for (i, (key, _)) in missing.iter().enumerate() {
        let value = if formatted.len() == missing.len() {
            formatted[i].clone()
        } else {
            key.to_string()
        };
        cache.insert(key.to_string(), value);
    }
}

fn format_history_time(value: &str) -> String {
    time_cache()
        .lock()
        .unwrap()
        .get(value)
        .cloned()
        .unwrap_or_else(|| value.to_string())
}

fn draw_history(out: &mut Stdout, state: &AppState, cols: u16, rows: u16) -> io::Result<()> {
    let accent = state.settings.accent_color();

    out.queue(MoveTo(0, 0))?;
    out.queue(SetForegroundColor(accent))?;
    out.queue(Print(format!("Download History  {} entries", state.history.len())))?;
    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    draw_notice(out, state, cols, 1)?;

    let footer_row = rows.saturating_sub(2);
    let first_row = 2u16;
    let available = footer_row.saturating_sub(first_row) as usize;
    let total = state.history.len();
    let max_start = total.saturating_sub(available);
    let start = state
        .history_cursor
        .saturating_sub(available.saturating_sub(1))
        .min(max_start);

    let visible: Vec<&str> = state
        .history
        .iter()
        .skip(start)
        .take(available)
        .map(|e| e.timestamp.as_str())
        .collect();
    prime_history_times(&visible);

    let date_w = 23usize;
    let status_w = 12usize;
    let mode_w = 9usize;
    let title_w = (cols as usize).saturating_sub(status_w + mode_w + date_w + 6);

    for r in first_row..footer_row {
        out.queue(MoveTo(0, r))?;
        out.queue(Clear(ClearType::CurrentLine))?;
    }

    for (display_index, entry) in state
        .history
        .iter()
        .skip(start)
        .take(available)
        .enumerate()
    {
        let row = first_row + display_index as u16;
        let selected = start + display_index == state.history_cursor;
        let status = entry.status.as_str();
        let status_color = if entry.status == "Completed" {
            Color::Green
        } else {
            Color::Red
        };
        let mode = fit(&entry.mode, mode_w);
        let title = fit(&entry.title, title_w.max(1));
        let date = fit(&format_history_time(&entry.timestamp), date_w);

        out.queue(MoveTo(0, row))?;
        if selected {
            out.queue(SetForegroundColor(accent))?;
            out.queue(Print("> "))?;
        } else {
            out.queue(Print("  "))?;
        }

        out.queue(SetForegroundColor(status_color))?;
        out.queue(Print(format!("{:<10}", status)))?;
        out.queue(SetForegroundColor(Color::Grey))?;
        out.queue(Print(format!(" {:<8}", mode)))?;
        out.queue(SetForegroundColor(if selected {
            Color::White
        } else {
            Color::Grey
        }))?;
        let title_end = 2 + 10 + 1 + 8 + title.chars().count();
        out.queue(Print(title))?;
        if title_end + date_w + 2 <= cols as usize {
            out.queue(MoveTo(cols.saturating_sub(date_w as u16), row))?;
            out.queue(SetForegroundColor(Color::DarkGrey))?;
            out.queue(Print(format!("{:>width$}", date, width = date_w)))?;
        }
        out.queue(ResetColor)?;
    }

    if state.history.is_empty() {
        out.queue(MoveTo(0, first_row))?;
        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print("No download history"))?;
        out.queue(ResetColor)?;
    }

    draw_footer(
        out,
        footer_row,
        cols,
        accent,
        &[
            ("↑/↓ W/S", "Select"),
            ("F", "Download again"),
            ("X", "Delete"),
            ("C", "Clear all"),
            ("F4/Esc", "Back"),
        ],
    )?;

    out.queue(MoveTo(0, rows.saturating_sub(1)))?;
    out.queue(Clear(ClearType::CurrentLine))?;
    if let Some(entry) = state.history.get(state.history_cursor) {
        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print(format!("URL: {}", fit(&entry.url, cols as usize))))?;
        out.queue(ResetColor)?;
    }

    out.queue(Hide)?;
    out.flush()?;
    Ok(())
}

fn draw_main(
    out: &mut Stdout,
    state: &AppState,
    cols: u16,
    rows: u16,
    spin: char,
) -> io::Result<()> {
    let accent = state.settings.accent_color();

    let tail_w = (cols as usize).saturating_sub(LABEL_W + 1);

    let fast_slots = state.settings.fast_downloads;

    out.queue(MoveTo(0, 0))?;
    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print("Select: "))?;

    let mode_str = match state.mode {
        Mode::Audio => {
            format!("{} (Audio)", state.settings.audio_format.to_uppercase())
        }
        Mode::Video => "MP4 (Video)".to_string(),
    };

    if state.mode == Mode::Audio {
        out.queue(SetForegroundColor(accent))?;
    } else {
        out.queue(SetForegroundColor(Color::Cyan))?;
    }

    out.queue(Print(mode_str))?;

    out.queue(SetForegroundColor(Color::White))?;
    out.queue(Print("   Fast: "))?;

    if state.fast_mode {
        out.queue(SetForegroundColor(Color::Yellow))?;
        out.queue(Print("ON "))?;

        out.queue(SetForegroundColor(Color::DarkGrey))?;

        out.queue(Print(format!("({} parallel)", fast_slots)))?;
    } else {
        out.queue(SetForegroundColor(Color::DarkGrey))?;
        out.queue(Print("OFF (queue)"))?;
    }

    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 1))?;
    out.queue(ResetColor)?;

    out.queue(Print("Please enter Youtube URL to start download "))?;

    out.queue(Print(match state.mode {
        Mode::Audio => "audio ",
        Mode::Video => "video ",
    }))?;

    out.queue(Print("or enter file path (.txt) with multiple link to"))?;

    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 2))?;

    let row2_text = if state.fast_mode {
        match state.mode {
            Mode::Audio => {
                format!("download multiple audio at once ({} Max)", fast_slots)
            }
            Mode::Video => {
                format!("download multiple video at once ({} Max)", fast_slots)
            }
        }
    } else {
        match state.mode {
            Mode::Audio => "download audio one by one (Queue mode)".to_string(),
            Mode::Video => "download video one by one (Queue mode)".to_string(),
        }
    };

    out.queue(Print(row2_text))?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(MoveTo(0, 3))?;
    out.queue(SetForegroundColor(Color::DarkGrey))?;

    out.queue(Print(fit(
        &format!("Output Path: {}", state.output_path.display()),
        cols as usize,
    )))?;

    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::UntilNewLine))?;

    draw_summary(out, state)?;

    draw_notice(out, state, cols, 5)?;

    let available_list_rows = list_capacity(rows);
    let total_downloads = state.downloads.len();

    let max_scroll = total_downloads.saturating_sub(available_list_rows);

    let display_offset = state.scroll_offset.min(max_scroll);

    let visible_downloads = state
        .downloads
        .iter()
        .skip(display_offset)
        .take(available_list_rows);

    let mut current_row = HEADER_LINES;

    for item in visible_downloads {
        out.queue(MoveTo(0, current_row))?;

        match item.status {
            DownloadStatus::Queued => {
                out.queue(SetForegroundColor(Color::DarkGrey))?;

                out.queue(Print(format!("{:<w$}", "Queued", w = LABEL_W)))?;

                out.queue(ResetColor)?;
                out.queue(Print(fit(&item.url, tail_w)))?;
            }

            DownloadStatus::Loading => {
                out.queue(SetForegroundColor(accent))?;
                out.queue(Print(format!("{} ", spin)))?;

                out.queue(ResetColor)?;

                out.queue(Print(format!("{:<w$}", "Loading…", w = LABEL_W - 2)))?;

                out.queue(SetForegroundColor(Color::DarkGrey))?;
                out.queue(Print(fit(&item.url, tail_w)))?;

                out.queue(ResetColor)?;
            }

            DownloadStatus::Downloading(pct) => {
                let title = item.title.as_deref().unwrap_or("Unknown Title");

                out.queue(SetForegroundColor(accent))?;
                out.queue(Print(format!("{} ", spin)))?;

                out.queue(ResetColor)?;

                let label = format!("Downloading… {:>5.1}%", pct);

                out.queue(Print(format!("{:<w$}", label, w = LABEL_W - 2)))?;

                out.queue(Print(fit(title, tail_w)))?;
            }

            DownloadStatus::Completed => {
                let title = item.title.as_deref().unwrap_or("Unknown Title");

                out.queue(SetForegroundColor(Color::Green))?;

                out.queue(Print(format!("{:<w$}", "✓ Download complete", w = LABEL_W)))?;

                out.queue(ResetColor)?;
                out.queue(Print(fit(title, tail_w)))?;
            }

            DownloadStatus::Error(ref err) => {
                let subject = item.title.as_deref().unwrap_or(item.url.as_str());

                out.queue(SetForegroundColor(Color::Red))?;

                out.queue(Print(format!("{:<w$}", "✗ Download failed", w = LABEL_W)))?;

                out.queue(ResetColor)?;

                if err.is_empty() || err.as_str() == "Failed" {
                    out.queue(Print(fit(subject, tail_w)))?;
                } else {
                    let reason = fit(err, tail_w);

                    let left = tail_w.saturating_sub(reason.chars().count() + 2);

                    out.queue(Print(reason))?;

                    if left > 8 {
                        out.queue(SetForegroundColor(Color::DarkGrey))?;

                        out.queue(Print(format!("  {}", fit(subject, left))))?;

                        out.queue(ResetColor)?;
                    }
                }
            }

            DownloadStatus::Cancelled => {
                let title = item.title.as_deref().unwrap_or(item.url.as_str());

                out.queue(SetForegroundColor(Color::DarkGrey))?;

                out.queue(Print(format!("{:<w$}", "CANCELLED", w = LABEL_W)))?;

                out.queue(ResetColor)?;
                out.queue(Print(fit(title, tail_w)))?;
            }
        }

        out.queue(Clear(ClearType::UntilNewLine))?;

        current_row += 1;
    }

    out.queue(ResetColor)?;

    let footer_row = rows.saturating_sub(2);

    if current_row < footer_row {
        for r in current_row..footer_row {
            out.queue(MoveTo(0, r))?;
            out.queue(Clear(ClearType::CurrentLine))?;
        }
    }

    draw_footer(
        out,
        footer_row,
        cols,
        accent,
        &[
            ("F5", "Mode"),
            ("F6", "Fast DL"),
            ("F4", "History"),
            ("F7", "Settings"),
            ("Enter", "Confirm"),
            ("Space", "Cancel"),
            ("Tab", "Path"),
            ("Ctrl+V", "Paste"),
        ],
    )?;

    let input_row = rows.saturating_sub(1);

    out.queue(MoveTo(0, input_row))?;
    out.queue(ResetColor)?;
    out.queue(Print(format!(">> {}", state.input_buffer)))?;

    out.queue(Clear(ClearType::UntilNewLine))?;

    out.queue(Show)?;
    out.flush()?;

    Ok(())
}