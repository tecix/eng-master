//! Installs en-Master for the current user without Flatpak: the binary, desktop entry, D-Bus
//! service, icons, metainfo, translations, autostart (for the daily reminder) and GNOME
//! keyboard shortcuts. The Flatpak build gets the same files from its manifest and uses the
//! GlobalShortcuts and Background portals instead.

use crate::paths;
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

include!(concat!(env!("OUT_DIR"), "/translations.rs"));

const DESKTOP: &str = include_str!("../data/io.github.tecix.eng_master.desktop");
const SERVICE: &str = include_str!("../data/io.github.tecix.eng_master.service");
const METAINFO: &str = include_str!("../data/io.github.tecix.eng_master.metainfo.xml");
const ICON: &str = include_str!("../data/icons/hicolor/scalable/apps/io.github.tecix.eng_master.svg");
const SYMBOLIC_ICON: &str = include_str!("../data/icons/hicolor/symbolic/apps/io.github.tecix.eng_master-symbolic.svg");

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const CUSTOM_BASE: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";

pub struct Shortcut {
    pub slot: &'static str,
    pub name: &'static str,
    pub binding: &'static str,
    pub args: &'static str,
}

pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut { slot: "enmaster", name: "en-Master: open on selection", binding: "<Super><Alt>e", args: "" },
    Shortcut {
        slot: "enmaster-coach",
        name: "en-Master: coach selection",
        binding: "<Super><Alt>c",
        args: "--action coach",
    },
    Shortcut { slot: "enmaster-drill", name: "en-Master: daily drill", binding: "<Super><Alt>d", args: "drill" },
];

fn gsettings(args: &[&str]) -> Result<String> {
    let out = Command::new("gsettings").args(args).output().context("gsettings is not available")?;
    if !out.status.success() {
        bail!("gsettings {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Strings inside single quotes of a GVariant text value, e.g. `['a', 'b']`.
fn quoted(value: &str) -> Vec<String> {
    value.split('\'').skip(1).step_by(2).map(str::to_string).collect()
}

fn variant_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|i| format!("'{}'", i.replace('\'', "\\'"))).collect();
    format!("[{}]", inner.join(", "))
}

/// Accelerator as (modifiers, key), so `<Alt><Super>e` equals `<Super><Alt>E`.
fn accel(text: &str) -> Option<(BTreeSet<String>, String)> {
    let mut mods = BTreeSet::new();
    let mut rest = text.trim();
    while let Some(stripped) = rest.strip_prefix('<') {
        let (name, after) = stripped.split_once('>')?;
        let name = match name.to_ascii_lowercase().as_str() {
            "primary" | "ctrl" | "control" => "control".to_string(),
            "mod4" => "super".to_string(),
            other => other.to_string(),
        };
        mods.insert(name);
        rest = after;
    }
    (!rest.is_empty()).then(|| (mods, rest.to_ascii_lowercase()))
}

/// Other bindings already using `binding` (any GNOME schema or custom shortcut).
fn conflicts(binding: &str, own_paths: &[String]) -> Vec<String> {
    let Some(wanted) = accel(binding) else { return Vec::new() };
    let mut found = Vec::new();
    if let Ok(all) = gsettings(&["list-recursively"]) {
        for line in all.lines() {
            if quoted(line).iter().any(|b| accel(b).as_ref() == Some(&wanted)) {
                found.push(line.split_whitespace().take(2).collect::<Vec<_>>().join(" "));
            }
        }
    }
    let list = gsettings(&["get", MEDIA_KEYS, "custom-keybindings"]).unwrap_or_default();
    for path in quoted(&list) {
        if own_paths.contains(&path) {
            continue;
        }
        let schema = format!("{CUSTOM_SCHEMA}:{path}");
        let bound = gsettings(&["get", &schema, "binding"]).unwrap_or_default();
        if quoted(&bound).first().and_then(|b| accel(b)).as_ref() == Some(&wanted) {
            let name = gsettings(&["get", &schema, "name"]).unwrap_or_default();
            found.push(format!("custom shortcut {name}"));
        }
    }
    found
}

fn data_home() -> PathBuf {
    dirs::data_dir().unwrap_or_default()
}

fn bin_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".local/bin/enmaster")
}

/// Files this installer writes, relative to their base directories.
fn installed_files() -> Vec<PathBuf> {
    let id = paths::APP_ID;
    let data = data_home();
    let mut files = vec![
        data.join(format!("applications/{id}.desktop")),
        data.join(format!("dbus-1/services/{id}.service")),
        data.join(format!("metainfo/{id}.metainfo.xml")),
        data.join(format!("icons/hicolor/scalable/apps/{id}.svg")),
        data.join(format!("icons/hicolor/symbolic/apps/{id}-symbolic.svg")),
        autostart_path(),
    ];
    for (lang, _) in TRANSLATIONS {
        files.push(paths::locale_dir().join(format!("{lang}/LC_MESSAGES/{}.mo", paths::GETTEXT_PACKAGE)));
    }
    files
}

fn autostart_path() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join(format!("autostart/{}.desktop", paths::APP_ID))
}

fn write(path: &Path, contents: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn install_binary() -> Result<PathBuf> {
    let current = std::env::current_exe()?.canonicalize()?;
    let target = bin_path();
    if current != target {
        std::fs::create_dir_all(target.parent().unwrap())?;
        let tmp = target.with_extension("new");
        std::fs::copy(&current, &tmp).with_context(|| format!("copying to {}", tmp.display()))?;
        std::fs::rename(&tmp, &target)?;
    }
    Ok(target)
}

/// Point `Exec=enmaster …` lines at the installed binary.
fn with_exe(text: &str, exe: &Path) -> String {
    text.lines()
        .map(|line| match line.strip_prefix("Exec=enmaster") {
            Some(rest) => format!("Exec={}{rest}", exe.display()),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn install_files(exe: &Path) -> Result<()> {
    let id = paths::APP_ID;
    let data = data_home();
    write(&data.join(format!("applications/{id}.desktop")), with_exe(DESKTOP, exe).as_bytes())?;
    write(&data.join(format!("dbus-1/services/{id}.service")), with_exe(SERVICE, exe).as_bytes())?;
    write(&data.join(format!("metainfo/{id}.metainfo.xml")), METAINFO.as_bytes())?;
    write(&data.join(format!("icons/hicolor/scalable/apps/{id}.svg")), ICON.as_bytes())?;
    write(&data.join(format!("icons/hicolor/symbolic/apps/{id}-symbolic.svg")), SYMBOLIC_ICON.as_bytes())?;
    for (lang, mo) in TRANSLATIONS {
        write(&paths::locale_dir().join(format!("{lang}/LC_MESSAGES/{}.mo", paths::GETTEXT_PACKAGE)), mo)?;
    }
    // Start in the background at login so the daily reminder and shortcuts respond at once.
    let autostart = format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec={} --background\nIcon={id}\nNoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        paths::APP_NAME,
        exe.display()
    );
    write(&autostart_path(), autostart.as_bytes())?;
    let _ = Command::new("gtk4-update-icon-cache").args(["-q", "-t", "-f"]).arg(data.join("icons/hicolor")).status();
    let _ = Command::new("update-desktop-database").arg(data.join("applications")).status();
    Ok(())
}

fn install_shortcuts(exe: &Path) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let own: Vec<String> = SHORTCUTS.iter().map(|s| format!("{CUSTOM_BASE}/{}/", s.slot)).collect();
    let mut list = quoted(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?);
    for (shortcut, path) in SHORTCUTS.iter().zip(&own) {
        let schema = format!("{CUSTOM_SCHEMA}:{path}");
        let command = format!("{} {}", exe.display(), shortcut.args).trim().to_string();
        gsettings(&["set", &schema, "name", shortcut.name])?;
        gsettings(&["set", &schema, "command", &command])?;
        gsettings(&["set", &schema, "binding", shortcut.binding])?;
        if !list.contains(path) {
            list.push(path.clone());
        }
        let clash = conflicts(shortcut.binding, &own);
        let mut note = format!("{} → {}", shortcut.binding, shortcut.name);
        if !clash.is_empty() {
            note.push_str(&format!("  ⚠ also used by: {}", clash.join(", ")));
        }
        notes.push(note);
    }
    gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &variant_list(&list)])?;
    Ok(notes)
}

/// Whether the GNOME shortcuts written by `install` are present.
pub fn shortcuts_installed() -> bool {
    let list = gsettings(&["get", MEDIA_KEYS, "custom-keybindings"]).unwrap_or_default();
    SHORTCUTS.iter().all(|s| list.contains(&format!("{CUSTOM_BASE}/{}/", s.slot)))
}

/// Add or refresh just the keyboard shortcuts (used by Preferences).
pub fn install_shortcuts_only() -> Result<Vec<String>> {
    install_shortcuts(&std::env::current_exe()?.canonicalize()?)
}

fn remove_shortcut_slots(slots: &[String]) -> Result<bool> {
    let paths: Vec<String> = slots.iter().map(|slot| format!("{CUSTOM_BASE}/{slot}/")).collect();
    let list = quoted(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?);
    if !list.iter().any(|p| paths.contains(p)) {
        return Ok(false);
    }
    let kept: Vec<String> = list.into_iter().filter(|p| !paths.contains(p)).collect();
    gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &variant_list(&kept)])?;
    for path in &paths {
        let _ = gsettings(&["reset-recursively", &format!("{CUSTOM_SCHEMA}:{path}")]);
    }
    Ok(true)
}

fn remove_units(name: &str) -> bool {
    let dir = dirs::config_dir().unwrap_or_default().join("systemd/user");
    let files = [dir.join(format!("{name}.service")), dir.join(format!("{name}.timer"))];
    if !files.iter().any(|f| f.exists()) {
        return false;
    }
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", &format!("{name}.timer")]).status();
    for file in &files {
        let _ = std::fs::remove_file(file);
    }
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    true
}

/// Remove what earlier versions installed: the "english-mastery" name, the `org.enmaster.App`
/// ID and the systemd timer that the in-app reminder replaced.
fn remove_old_installs() -> Result<Vec<String>> {
    let legacy = paths::LEGACY_NAME;
    let mut removed = Vec::new();
    if remove_shortcut_slots(&[legacy.to_string(), format!("{legacy}-coach")])? {
        removed.push("english-mastery shortcuts".to_string());
    }
    for unit in [format!("{legacy}-drill"), "enmaster-drill".to_string()] {
        if remove_units(&unit) {
            removed.push(format!("{unit}.timer"));
        }
    }
    for old_id in ["org.englishmastery.Coach", "org.enmaster.App"] {
        if std::fs::remove_file(data_home().join(format!("applications/{old_id}.desktop"))).is_ok() {
            removed.push(format!("{old_id}.desktop"));
        }
    }
    let old_bin = bin_path().with_file_name(legacy);
    if old_bin.exists() {
        let _ = Command::new(&old_bin).arg("quit").status();
        std::fs::remove_file(&old_bin)?;
        removed.push(old_bin.display().to_string());
    }
    Ok(removed)
}

pub fn install() -> Result<()> {
    if paths::sandboxed() {
        bail!("inside Flatpak, en-Master is installed by Flatpak");
    }
    let removed = remove_old_installs()?;
    if !removed.is_empty() {
        println!("Removed from earlier versions: {}", removed.join(", "));
    }
    let exe = install_binary()?;
    println!("Installed {}", exe.display());
    // A resident instance of an older build would keep answering the shortcuts.
    let _ = Command::new(&exe).arg("quit").status();
    install_files(&exe)?;
    println!("Installed the desktop entry, icons, translations and autostart entry.");
    println!("Keyboard shortcuts (Settings → Keyboard → Custom Shortcuts):");
    for note in install_shortcuts(&exe)? {
        println!("  {note}");
    }
    let _ = Command::new(&exe).arg("--background").spawn();
    println!("en-Master is running in the background for reminders. Open it with Super+Alt+E.");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let slots: Vec<String> = SHORTCUTS.iter().map(|s| s.slot.to_string()).collect();
    remove_shortcut_slots(&slots)?;
    let _ = remove_old_installs();
    let _ = Command::new(bin_path()).arg("quit").status();
    for file in installed_files() {
        let _ = std::fs::remove_file(file);
    }
    let _ = std::fs::remove_file(bin_path());
    println!("Removed en-Master's shortcuts, files and {}.", bin_path().display());
    println!(
        "Your settings ({}) and learning log ({}) were kept.",
        paths::config_dir().display(),
        paths::data_dir().display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accelerators_compare_regardless_of_order_and_case() {
        assert_eq!(accel("<Super><Alt>e"), accel("<Alt><Super>E"));
        assert_eq!(accel("<Primary>v"), accel("<Control>v"));
        assert_ne!(accel("<Super><Alt>e"), accel("<Super>e"));
        assert_eq!(accel(""), None);
    }

    #[test]
    fn gvariant_lists_round_trip() {
        let items = vec!["/a/".to_string(), "/b/".to_string()];
        assert_eq!(quoted(&variant_list(&items)), items);
        assert!(quoted("@as []").is_empty());
    }

    #[test]
    fn exec_lines_point_at_the_binary() {
        let out = with_exe("Name=x\nExec=enmaster drill\nExec=enmaster\n", Path::new("/home/u/.local/bin/enmaster"));
        assert_eq!(out, "Name=x\nExec=/home/u/.local/bin/enmaster drill\nExec=/home/u/.local/bin/enmaster\n");
    }
}
