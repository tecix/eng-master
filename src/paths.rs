//! Locations of config, data and credential-lock files, plus private file helpers.

use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "io.github.tecix.eng_master";
pub const APP_NAME: &str = "en-Master";
pub const RESOURCE_PATH: &str = "/io/github/tecix/eng_master";
pub const GETTEXT_PACKAGE: &str = "enmaster";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const WEBSITE: &str = "https://github.com/tecix/eng-master";
pub const ISSUES: &str = "https://github.com/tecix/eng-master/issues";

/// Running inside Flatpak (or Snap): use portals instead of host tools.
pub fn sandboxed() -> bool {
    ashpd::is_sandboxed()
}

/// Where compiled translations live: `/app/share/locale` in Flatpak, `~/.local/share/locale` otherwise.
pub fn locale_dir() -> PathBuf {
    if let Some(dir) = option_env!("ENMASTER_LOCALEDIR") {
        return dir.into();
    }
    if sandboxed() {
        return PathBuf::from("/app/share/locale");
    }
    dirs::data_dir().unwrap_or_else(|| home().join(".local/share")).join("locale")
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.config/enmaster` (override with `ENMASTER_CONFIG_DIR`).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ENMASTER_CONFIG_DIR") {
        return dir.into();
    }
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join("enmaster")
}

/// `~/.local/share/enmaster` (override with `ENMASTER_DATA_DIR`).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ENMASTER_DATA_DIR") {
        return dir.into();
    }
    dirs::data_dir().unwrap_or_else(|| home().join(".local/share")).join("enmaster")
}

/// The app's name before it became en-Master.
pub const LEGACY_NAME: &str = "english-mastery";

/// Move settings and the learning log from the old `english-mastery` folders, once.
pub fn migrate_legacy() {
    if std::env::var_os("ENMASTER_CONFIG_DIR").is_some() || std::env::var_os("ENMASTER_DATA_DIR").is_some() {
        return;
    }
    for (base, new) in [(dirs::config_dir(), config_dir()), (dirs::data_dir(), data_dir())] {
        let Some(old) = base.map(|b| b.join(LEGACY_NAME)) else { continue };
        if old.is_dir() && !new.exists() {
            let _ = fs::rename(&old, &new);
        }
    }
}

pub fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

/// Atomically replace `path` with `contents`, readable only by the user.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Append one line to a user-only file.
pub fn append_private(path: &Path, line: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("appending to {}", path.display()))?;
    writeln!(file, "{line}")?;
    Ok(())
}
