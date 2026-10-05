use crossterm::{
    cursor::{Hide, MoveTo, Show},
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType},
    ExecutableCommand,
};
use std::io::{self, stdout, Stdout, Write};

use crate::settings::ROW_COUNT;
use crate::{updater, AppScreen, AppState, DownloadStatus, Mode};

/// Rows above the download list: 4 header lines, summary line, notice line.
pub const HEADER_LINES: u16 = 6;
/// Rows below the download list: key hints + input.
pub const FOOTER_LINES: u16 = 2;

/// Width of the status column; every row's title/URL starts right after it.
const LABEL_W: usize = 23;

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Number of download rows that fit on a terminal with `rows` lines.
pub fn list_capacity(rows: u16) -> usize {
    rows.saturating_sub(HEADER_LINES + FOOTER_LINES) as usize
}

/// Truncate to `max` characters, ending with an ellipsis when cut.
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

fn draw_footer(out: &mut Stdout, row: u16, cols: u16, accent: Color, items: &[(&str, &str)]) -> io::Result<()> {
    out.execute(MoveTo(0, row))?;
    let mut used = 0usize;
    for (key, label) in items {
        let need = key.chars().count() + 1 + label.chars().count() + 2;
        if used + need > cols as usize {
            break;
        }
        out.execute(SetForegroundColor(accent))?;
        out.execute(Print(*key))?;
        out.execute(SetForegroundColor(Color::DarkGrey))?;
        out.execute(Print(format!(" {}  ", label)))?;
        used += need;
    }
    out.execute(ResetColor)?;
    out.execute(Clear(ClearType::UntilNewLine))?;
    Ok(())
}

pub fn draw(state: &AppState) -> io::Result<()> {
    let mut out = stdout();
    out.execute(Hide)?;

    let (cols, rows) = crossterm::terminal::size()?;
    let spin = SPINNER[state.spinner_frame % SPINNER.len()];

    match state.screen {
        AppScreen::Installing => draw_installing(&mut out, state, rows, spin),
        AppScreen::Settings => draw_settings(&mut out, state, cols, rows),
        AppScreen::UpdatePrompt => draw_update_prompt(&mut out, state, rows),
        AppScreen::Updating => draw_updating(&mut out, state, rows, spin),
        AppScreen::Main => draw_main(&mut out, state, cols, rows, spin),
    }
}

fn draw_status_screen(out: &mut Stdout, rows: u16, text: &str) -> io::Result<()> {
    out.execute(MoveTo(0, 0))?;
    out.execute(SetForegroundColor(Color::White))?;
    out.execute(Print(text))?;
    out.execute(Clear(ClearType::UntilNewLine))?;
    out.execute(Print("\n"))?;
    out.execute(ResetColor)?;

    for r in 1..rows {
        out.execute(MoveTo(0, r))?;
        out.execute(Clear(ClearType::CurrentLine))?;
    }

    out.execute(Show)?;
    out.flush()?;
    Ok(())
}

fn draw_installing(out: &mut Stdout, state: &AppState, rows: u16, spin: char) -> io::Result<()> {
    let text = match &state.loading_text {
        Some(t) => format!("{} {}", spin, t),
        None => format!(
            "{} Installing requirements... ({:.1}% Completed)",
            spin, state.install_progress
        ),
    };
    draw_status_screen(out, rows, &text)
}

fn draw_updating(out: &mut Stdout, state: &AppState, rows: u16, spin: char) -> io::Result<()> {
    let version = state.pending_update.as_ref().map(|r| r.version.as_str()).unwrap_or("");
    let text = match &state.loading_text {
        Some(t) => format!("{} {}", spin, t),
        None => format!(
            "{} Updating Celestial to v{}... ({:.1}%)",
            spin, version, state.install_progress
        ),
    };
    draw_status_screen(out, rows, &text)
}

fn draw_update_prompt(out: &mut Stdout, state: &AppState, rows: u16) -> io::Result<()> {
    let accent = state.settings.accent_color();
    let version = state.pending_update.as_ref().map(|r| r.version.as_str()).unwrap_or("");

    out.execute(MoveTo(0, 0))?;
    out.execute(SetForegroundColor(accent))?;
    out.execute(Print("Update available"))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    out.execute(MoveTo(0, 2))?;
    out.execute(SetForegroundColor(Color::White))?;
    out.execute(Print(format!(
        "Celestial v{} is available (you have v{}).",
        version,
        updater::CURRENT
    )))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    out.execute(MoveTo(0, 3))?;
    out.execute(SetForegroundColor(Color::DarkGrey))?;
    out.execute(Print("Your settings are kept. Updating is optional."))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    out.execute(MoveTo(0, 5))?;
    out.execute(SetForegroundColor(accent))?;
    out.execute(Print("U"))?;
    out.execute(SetForegroundColor(Color::Grey))?;
    out.execute(Print(" Update now    "))?;
    out.execute(SetForegroundColor(accent))?;
    out.execute(Print("Enter"))?;
    out.execute(SetForegroundColor(Color::Grey))?;
    out.execute(Print(" Continue"))?;
    out.execute(ResetColor)?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    for r in [1u16, 4] {
        out.execute(MoveTo(0, r))?;
        out.execute(Clear(ClearType::CurrentLine))?;
    }
    for r in 6..rows {
        out.execute(MoveTo(0, r))?;
        out.execute(Clear(ClearType::CurrentLine))?;
    }

    out.execute(Hide)?;
    out.flush()?;
    Ok(())
}

fn draw_settings(out: &mut Stdout, state: &AppState, cols: u16, rows: u16) -> io::Result<()> {
    let accent = state.settings.accent_color();
    out.execute(MoveTo(0, 0))?;
    out.execute(SetForegroundColor(accent))?;
    out.execute(Print("Settings"))?;
    out.execute(SetForegroundColor(Color::DarkGrey))?;
    out.execute(Print("   changes are saved automatically"))?;
    out.execute(ResetColor)?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    out.execute(MoveTo(0, 1))?;
    out.execute(Clear(ClearType::CurrentLine))?;

    let hint_w = (cols as usize).saturating_sub(2 + 22 + 18);
    for (i, (label, value, hint)) in state.settings.rows().iter().enumerate() {
        let selected = i == state.settings_cursor;
        out.execute(MoveTo(0, 2 + i as u16))?;
        if selected {
            out.execute(SetForegroundColor(accent))?;
            out.execute(Print("> "))?;
            out.execute(SetForegroundColor(Color::White))?;
        } else {
            out.execute(ResetColor)?;
            out.execute(Print("  "))?;
            out.execute(SetForegroundColor(Color::Grey))?;
        }
        out.execute(Print(format!("{:<22}", label)))?;

        let shown = if selected { format!("‹ {} ›", value) } else { format!("  {}  ", value) };
        out.execute(SetForegroundColor(if selected { accent } else { Color::White }))?;
        out.execute(Print(format!("{:<18}", shown)))?;

        out.execute(SetForegroundColor(Color::DarkGrey))?;
        out.execute(Print(fit(hint, hint_w)))?;
        out.execute(ResetColor)?;
        out.execute(Clear(ClearType::UntilNewLine))?;
    }

    let footer_row = rows.saturating_sub(2);
    for r in (2 + ROW_COUNT as u16)..footer_row {
        out.execute(MoveTo(0, r))?;
        out.execute(Clear(ClearType::CurrentLine))?;
    }

    // Credit line under the settings list
    let credit_row = 3 + ROW_COUNT as u16;
    if credit_row < footer_row {
        out.execute(MoveTo(2, credit_row))?;
        out.execute(SetForegroundColor(Color::DarkGrey))?;
        out.execute(Print(format!("Celestial v{}  |  Made by ", updater::CURRENT)))?;
        out.execute(SetForegroundColor(accent))?;
        out.execute(Print("@kradengdeng"))?;
        out.execute(ResetColor)?;
    }

    draw_footer(
        out,
        footer_row,
        cols,
        accent,
        &[("↑/↓ W/S", "Select"), ("←/→ A/D", "Change"), ("F7/Esc", "Back")],
    )?;

    out.execute(MoveTo(0, rows.saturating_sub(1)))?;
    out.execute(Clear(ClearType::CurrentLine))?;

    out.execute(Hide)?;
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

    out.execute(MoveTo(0, 4))?;
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
                out.execute(SetForegroundColor(color))?;
                out.execute(Print(format!("{} {}", label, count)))?;
                out.execute(SetForegroundColor(Color::DarkGrey))?;
                out.execute(Print("  "))?;
            }
        }
        out.execute(SetForegroundColor(Color::DarkGrey))?;
        out.execute(Print(format!("({} total)", state.downloads.len())))?;
    }
    out.execute(ResetColor)?;
    out.execute(Clear(ClearType::UntilNewLine))?;
    Ok(())
}

fn draw_main(out: &mut Stdout, state: &AppState, cols: u16, rows: u16, spin: char) -> io::Result<()> {
    let accent = state.settings.accent_color();
    let tail_w = (cols as usize).saturating_sub(LABEL_W + 1);
    let audio_slots = state.settings.fast_audio_slots;
    let video_slots = state.settings.fast_video_slots;

    // Header (Row 0)
    out.execute(MoveTo(0, 0))?;
    out.execute(SetForegroundColor(Color::White))?;
    out.execute(Print("Select: "))?;
    let mode_str = match state.mode {
        Mode::Audio => format!("{} (Audio)", state.settings.audio_format.to_uppercase()),
        Mode::Video => "MP4 (Video)".to_string(),
    };
    if state.mode == Mode::Audio {
        out.execute(SetForegroundColor(accent))?;
    } else {
        out.execute(SetForegroundColor(Color::Cyan))?;
    }
    out.execute(Print(mode_str))?;
    // Fast mode indicator
    out.execute(SetForegroundColor(Color::White))?;
    out.execute(Print("   Fast: "))?;
    if state.fast_mode {
        out.execute(SetForegroundColor(Color::Yellow))?;
        out.execute(Print("ON "))?;
        out.execute(SetForegroundColor(Color::DarkGrey))?;
        let limit = match state.mode {
            Mode::Audio => format!("({} parallel)", audio_slots),
            Mode::Video => format!("({} parallel)", video_slots),
        };
        out.execute(Print(limit))?;
    } else {
        out.execute(SetForegroundColor(Color::DarkGrey))?;
        out.execute(Print("OFF (queue)"))?;
    }
    out.execute(Clear(ClearType::UntilNewLine))?;

    // Row 1
    out.execute(MoveTo(0, 1))?;
    out.execute(ResetColor)?;
    out.execute(Print("Please enter Youtube URL to start download "))?;
    out.execute(Print(match state.mode {
        Mode::Audio => "audio ",
        Mode::Video => "video ",
    }))?;
    out.execute(Print("or enter file path (.txt) with multiple link to"))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    // Row 2
    out.execute(MoveTo(0, 2))?;
    let row2_text = if state.fast_mode {
        match state.mode {
            Mode::Audio => format!("download multiple audio at once ({} Max)", audio_slots),
            Mode::Video => format!("download multiple video at once ({} Max)", video_slots),
        }
    } else {
        match state.mode {
            Mode::Audio => "download audio one by one (Queue mode)".to_string(),
            Mode::Video => "download video one by one (Queue mode)".to_string(),
        }
    };
    out.execute(Print(row2_text))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    // Row 3: Output path
    out.execute(MoveTo(0, 3))?;
    out.execute(SetForegroundColor(Color::DarkGrey))?;
    out.execute(Print(fit(
        &format!("Output Path: {}", state.output_path.display()),
        cols as usize,
    )))?;
    out.execute(ResetColor)?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    // Row 4: Summary (recomputed on every redraw)
    draw_summary(out, state)?;

    // Row 5: Notice
    out.execute(MoveTo(0, 5))?;
    if let Some(text) = &state.notice {
        if state.notice_busy {
            out.execute(SetForegroundColor(accent))?;
            out.execute(Print(format!("{} ", spin)))?;
            out.execute(SetForegroundColor(Color::Grey))?;
            out.execute(Print(fit(text, (cols as usize).saturating_sub(2))))?;
        } else {
            out.execute(SetForegroundColor(Color::Grey))?;
            out.execute(Print(fit(text, cols as usize)))?;
        }
        out.execute(ResetColor)?;
    }
    out.execute(Clear(ClearType::UntilNewLine))?;

    // Downloads
    let available_list_rows = list_capacity(rows);
    let total_downloads = state.downloads.len();
    let max_scroll = total_downloads.saturating_sub(available_list_rows);
    let display_offset = state.scroll_offset.min(max_scroll);

    let visible_downloads = state.downloads.iter().skip(display_offset).take(available_list_rows);

    let mut current_row = HEADER_LINES;
    for item in visible_downloads {
        out.execute(MoveTo(0, current_row))?;
        match item.status {
            DownloadStatus::Queued => {
                out.execute(SetForegroundColor(Color::DarkGrey))?;
                out.execute(Print(format!("{:<w$}", "Queued", w = LABEL_W)))?;
                out.execute(ResetColor)?;
                out.execute(Print(fit(&item.url, tail_w)))?;
            }
            DownloadStatus::Loading => {
                out.execute(SetForegroundColor(accent))?;
                out.execute(Print(format!("{} ", spin)))?;
                out.execute(ResetColor)?;
                out.execute(Print(format!("{:<w$}", "Loading…", w = LABEL_W - 2)))?;
                out.execute(SetForegroundColor(Color::DarkGrey))?;
                out.execute(Print(fit(&item.url, tail_w)))?;
                out.execute(ResetColor)?;
            }
            DownloadStatus::Downloading(pct) => {
                let title = item.title.as_deref().unwrap_or("Unknown Title");
                out.execute(SetForegroundColor(accent))?;
                out.execute(Print(format!("{} ", spin)))?;
                out.execute(ResetColor)?;
                let label = format!("Downloading… {:>5.1}%", pct);
                out.execute(Print(format!("{:<w$}", label, w = LABEL_W - 2)))?;
                out.execute(Print(fit(title, tail_w)))?;
            }
            DownloadStatus::Completed => {
                let title = item.title.as_deref().unwrap_or("Unknown Title");
                out.execute(SetForegroundColor(Color::Green))?;
                out.execute(Print(format!("{:<w$}", "✓ Download complete", w = LABEL_W)))?;
                out.execute(ResetColor)?;
                out.execute(Print(fit(title, tail_w)))?;
            }
            DownloadStatus::Error(ref err) => {
                let subject = item.title.as_deref().unwrap_or(item.url.as_str());
                out.execute(SetForegroundColor(Color::Red))?;
                out.execute(Print(format!("{:<w$}", "✗ Download failed", w = LABEL_W)))?;
                out.execute(ResetColor)?;
                if err.is_empty() || err.as_str() == "Failed" {
                    out.execute(Print(fit(subject, tail_w)))?;
                } else {
                    // Reason first, then the video it belongs to.
                    let reason = fit(err, tail_w);
                    let left = tail_w.saturating_sub(reason.chars().count() + 2);
                    out.execute(Print(reason))?;
                    if left > 8 {
                        out.execute(SetForegroundColor(Color::DarkGrey))?;
                        out.execute(Print(format!("  {}", fit(subject, left))))?;
                        out.execute(ResetColor)?;
                    }
                }
            }
            DownloadStatus::Cancelled => {
                let title = item.title.as_deref().unwrap_or(item.url.as_str());
                out.execute(SetForegroundColor(Color::DarkGrey))?;
                out.execute(Print(format!("{:<w$}", "CANCELLED", w = LABEL_W)))?;
                out.execute(ResetColor)?;
                out.execute(Print(fit(title, tail_w)))?;
            }
        }
        out.execute(Clear(ClearType::UntilNewLine))?;
        current_row += 1;
    }
    out.execute(ResetColor)?;

    // Clear empty lines between downloads and footer
    let footer_row = rows.saturating_sub(2);
    if current_row < footer_row {
        for r in current_row..footer_row {
            out.execute(MoveTo(0, r))?;
            out.execute(Clear(ClearType::CurrentLine))?;
        }
    }

    // Footer (Row rows - 2)
    draw_footer(
        out,
        footer_row,
        cols,
        accent,
        &[
            ("F5", "Mode"),
            ("F6", "Fast DL"),
            ("F7", "Settings"),
            ("F8", "yt-dlp"),
            ("F9", "Update"),
            ("Enter", "Confirm"),
            ("Space", "Cancel"),
            ("Tab", "Path"),
            ("Ctrl+V", "Paste"),
        ],
    )?;

    // Input (Row rows - 1)
    let input_row = rows.saturating_sub(1);
    out.execute(MoveTo(0, input_row))?;
    out.execute(ResetColor)?;
    out.execute(Print(format!(">> {}", state.input_buffer)))?;
    out.execute(Clear(ClearType::UntilNewLine))?;

    out.execute(Show)?;
    out.flush()?;
    Ok(())
}
