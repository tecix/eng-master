//! Reading the selected text and pasting results back on GNOME Wayland.
//!
//! A shortcut-launched app has no keyboard focus yet, so it can't read the Wayland PRIMARY
//! selection itself. Mutter mirrors selections to XWayland, where any X client may read them
//! without focus (the same trick the TTS shortcuts on this machine use with `xsel`).

use crate::config::Config;
use anyhow::{Result, bail};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Selection,
    Clipboard,
    Empty,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::Selection => "From your selection",
            Source::Clipboard => "From the clipboard",
            Source::Empty => "Type or paste your text",
        }
    }
}

const READ_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_BYTES: usize = 256 * 1024;

fn read_command(argv: &[&str]) -> Option<String> {
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.by_ref().take(MAX_BYTES as u64).read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + READ_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text = String::from_utf8_lossy(&reader.join().ok()?).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn first_of(commands: &[&[&str]]) -> Option<String> {
    commands.iter().find_map(|argv| read_command(argv))
}

pub fn read_primary() -> Option<String> {
    first_of(&[
        &["xsel", "--primary", "--output"],
        &["xclip", "-out", "-selection", "primary"],
        &["wl-paste", "--primary", "--no-newline"],
    ])
}

pub fn read_clipboard() -> Option<String> {
    first_of(&[
        &["xsel", "--clipboard", "--output"],
        &["xclip", "-out", "-selection", "clipboard"],
        &["wl-paste", "--no-newline"],
    ])
}

/// The selected text, else the clipboard.
pub fn capture() -> (String, Source) {
    if let Some(text) = read_primary() {
        return (text, Source::Selection);
    }
    if let Some(text) = read_clipboard() {
        return (text, Source::Clipboard);
    }
    (String::new(), Source::Empty)
}

/// Send the paste keystroke to whichever window has focus now.
pub fn paste(cfg: &Config) -> Result<()> {
    let Some((program, args)) = cfg.paste_command.split_first() else {
        bail!("paste is turned off");
    };
    // Shortcut-launched processes may not have ~/.local/bin on PATH.
    let local = dirs::home_dir().unwrap_or_default().join(".local/bin").join(program);
    let resolved = if !program.contains('/') && local.exists() { local } else { program.into() };
    let mut command = Command::new(resolved);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Shortcut-launched processes may not inherit the shell's YDOTOOL_SOCKET.
    if std::env::var_os("YDOTOOL_SOCKET").is_none() && std::path::Path::new("/tmp/.ydotool_socket").exists() {
        command.env("YDOTOOL_SOCKET", "/tmp/.ydotool_socket");
    }
    let status = command.status()?;
    if !status.success() {
        bail!("{program} failed ({status})");
    }
    Ok(())
}
