//! Import the previous app's workspace and native window frame once.

use std::{collections::BTreeMap, error::Error, fs, io, path::Path};

use rune::prefs::Prefs;
use serde_json::Value;

use super::{KEY, RECENTS_KEY};
use crate::{APP_ID, frame, panels::WIDTHS_KEY};

const LEGACY_ID: &str = "local.git-viewer";
const MARKER: &str = "legacy_preferences_migrated";
const WORKSPACE_KEYS: [&str; 3] = [KEY, RECENTS_KEY, WIDTHS_KEY];

/// Prepare the desktop preferences before the first window reads its frame.
pub(crate) fn run() -> Result<Prefs, String> {
    let mut current = Prefs::open(APP_ID);
    if let Some(path) = current.path().and_then(Path::parent) {
        let legacy = path.with_file_name(LEGACY_ID).join("prefs.json");
        let frame_domains =
            (cfg!(target_os = "macos") && !frame::isolated()).then_some((LEGACY_ID, APP_ID));
        migrate(&mut current, &legacy, frame_domains)
            .map_err(|error| format!("Couldn’t migrate preferences: {error}"))?;
    }
    Ok(current)
}

fn migrate(
    current: &mut Prefs,
    legacy_path: &Path,
    frame_domains: Option<(&str, &str)>,
) -> Result<(), Box<dyn Error>> {
    let marker = current.get::<bool>(MARKER);
    if marker == Some(true) {
        return Ok(());
    }
    let current_frame = match frame_domains {
        Some((_, domain)) => read_frame(domain)?,
        None => None,
    };
    if marker.is_none() {
        if current_frame.is_some() || WORKSPACE_KEYS.iter().any(|key| current.contains(key)) {
            current.set(MARKER, &true)?;
            return Ok(());
        }
        // Checkpoint before writing either store so an interrupted migration
        // can finish copying missing keys on the next launch.
        current.set(MARKER, &false)?;
    }
    // Prefs::at repairs invalid files. Read the source directly to leave it intact.
    let legacy: BTreeMap<String, Value> = match fs::read(legacy_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error.into()),
    };
    for key in WORKSPACE_KEYS {
        if !current.contains(key)
            && let Some(value) = legacy.get(key)
        {
            current.set(key, value)?;
        }
    }
    if current_frame.is_none()
        && let Some((old, new)) = frame_domains
        && let Some(value) = read_frame(old)?
    {
        write_frame(new, &value)?;
    }
    current.set(MARKER, &true)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn frame_key() -> String {
    format!("NSWindow Frame {}", frame::FRAME_NAME)
}

#[cfg(target_os = "macos")]
fn read_frame(domain: &str) -> io::Result<Option<String>> {
    let result = defaults().args(["read", domain, &frame_key()]).output()?;
    if result.status.success() {
        let mut frame = String::from_utf8(result.stdout).map_err(io::Error::other)?;
        if frame.ends_with('\n') {
            frame.pop();
        }
        return Ok(Some(frame));
    }
    let error = String::from_utf8_lossy(&result.stderr);
    if result.status.code() == Some(1) && error.trim_end().ends_with("does not exist") {
        return Ok(None);
    }
    Err(io::Error::other(error.trim()))
}

#[cfg(not(target_os = "macos"))]
fn read_frame(_: &str) -> io::Result<Option<String>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn write_frame(domain: &str, frame: &str) -> io::Result<()> {
    let result = defaults()
        .args(["write", domain, &frame_key(), "-string", frame])
        .output()?;
    if result.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&result.stderr).trim(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
fn write_frame(_: &str, _: &str) -> io::Result<()> {
    Ok(())
}

// Use macOS's preferences tool to update one NSUserDefaults key, including
// synchronizing it to disk, without rewriting the rest of either domain.
#[cfg(target_os = "macos")]
fn defaults() -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/defaults");
    command.env("LC_ALL", "C");
    command
}

#[cfg(test)]
mod tests;
