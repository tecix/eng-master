//! Installs the binary, a desktop entry, GNOME keyboard shortcuts and the daily drill timer.

use crate::paths;
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const CUSTOM_BASE: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";
const TIMER: &str = "enmaster-drill";

pub struct Shortcut {
    pub slot: &'static str,
    pub name: &'static str,
    pub binding: &'static str,
    pub args: &'static str,
}

pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut { slot: "enmaster", name: "en-Master: open on selection", binding: "<Super><Alt>e", args: "" },
    Shortcut { slot: "enmaster-coach", name: "en-Master: coach selection", binding: "<Super><Alt>c", args: "--action coach" },
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

fn bin_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".local/bin/enmaster")
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

fn install_desktop_file(exe: &Path) -> Result<PathBuf> {
    let dir = dirs::data_dir().unwrap_or_default().join("applications");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.desktop", paths::APP_ID));
    let exe = exe.display();
    std::fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Application\nName={name}\nComment=Practise English writing on any selected text\n\
             Exec={exe}\nIcon=accessories-dictionary\nTerminal=false\nCategories=Education;Languages;Office;\n\
             Keywords=English;Writing;Grammar;Coach;\nStartupNotify=true\nActions=drill;progress;\n\n\
             [Desktop Action drill]\nName=Daily drill\nExec={exe} drill\n\n\
             [Desktop Action progress]\nName=Progress\nExec={exe} progress\n",
            name = paths::APP_NAME
        ),
    )?;
    Ok(path)
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

fn systemd_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("systemd/user")
}

fn install_timer(exe: &Path, time: &str) -> Result<()> {
    let dir = systemd_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{TIMER}.service")),
        format!(
            "[Unit]\nDescription=en-Master daily drill reminder\n\n\
             [Service]\nType=simple\nExecStart={} drill-reminder\n",
            exe.display()
        ),
    )?;
    std::fs::write(
        dir.join(format!("{TIMER}.timer")),
        format!(
            "[Unit]\nDescription=en-Master daily drill reminder\n\n\
             [Timer]\nOnCalendar=*-*-* {time}:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
        ),
    )?;
    let reload = Command::new("systemctl").args(["--user", "daemon-reload"]).status()?;
    let enable = Command::new("systemctl").args(["--user", "enable", "--now", &format!("{TIMER}.timer")]).status()?;
    if !reload.success() || !enable.success() {
        bail!("systemctl could not enable {TIMER}.timer");
    }
    Ok(())
}

/// Remove what an install under the old name ("english-mastery") left behind.
fn remove_legacy() -> Result<Vec<String>> {
    let legacy = paths::LEGACY_NAME;
    let mut removed = Vec::new();
    let old_paths: Vec<String> =
        [legacy.to_string(), format!("{legacy}-coach")].iter().map(|slot| format!("{CUSTOM_BASE}/{slot}/")).collect();
    let list = quoted(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?);
    if list.iter().any(|p| old_paths.contains(p)) {
        let kept: Vec<String> = list.into_iter().filter(|p| !old_paths.contains(p)).collect();
        gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &variant_list(&kept)])?;
        for path in &old_paths {
            let _ = gsettings(&["reset-recursively", &format!("{CUSTOM_SCHEMA}:{path}")]);
        }
        removed.push("old keyboard shortcuts".to_string());
    }
    let timer = format!("{legacy}-drill");
    let unit_files = [systemd_dir().join(format!("{timer}.service")), systemd_dir().join(format!("{timer}.timer"))];
    if unit_files.iter().any(|f| f.exists()) {
        let _ = Command::new("systemctl").args(["--user", "disable", "--now", &format!("{timer}.timer")]).status();
        for file in &unit_files {
            let _ = std::fs::remove_file(file);
        }
        let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        removed.push(format!("{timer}.timer"));
    }
    let desktop = dirs::data_dir().unwrap_or_default().join("applications/org.englishmastery.Coach.desktop");
    if std::fs::remove_file(&desktop).is_ok() {
        removed.push("old desktop entry".to_string());
    }
    let old_bin = bin_path().with_file_name(legacy);
    if old_bin.exists() {
        let _ = Command::new(&old_bin).arg("quit").status();
        std::fs::remove_file(&old_bin)?;
        removed.push(old_bin.display().to_string());
    }
    Ok(removed)
}

pub fn install(with_timer: bool, time: &str) -> Result<()> {
    if accel_time(time).is_none() {
        bail!("drill time must look like 09:30");
    }
    let removed = remove_legacy()?;
    if !removed.is_empty() {
        println!("Removed the old english-mastery install: {}", removed.join(", "));
    }
    let exe = install_binary()?;
    println!("Installed {}", exe.display());
    // A resident instance of an older build would keep answering the shortcut.
    let _ = Command::new(&exe).arg("quit").status();
    println!("Desktop entry: {}", install_desktop_file(&exe)?.display());
    println!("Keyboard shortcuts (Settings → Keyboard → Custom Shortcuts):");
    for note in install_shortcuts(&exe)? {
        println!("  {note}");
    }
    if with_timer {
        install_timer(&exe, time)?;
        println!("Daily drill reminder at {time} ({TIMER}.timer)");
    }
    Ok(())
}

fn accel_time(time: &str) -> Option<(u32, u32)> {
    let (h, m) = time.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60 && time.len() == 5).then_some((h, m))
}

pub fn uninstall() -> Result<()> {
    let own: Vec<String> = SHORTCUTS.iter().map(|s| format!("{CUSTOM_BASE}/{}/", s.slot)).collect();
    let list: Vec<String> = quoted(&gsettings(&["get", MEDIA_KEYS, "custom-keybindings"])?)
        .into_iter()
        .filter(|p| !own.contains(p))
        .collect();
    gsettings(&["set", MEDIA_KEYS, "custom-keybindings", &variant_list(&list)])?;
    for path in &own {
        let _ = gsettings(&["reset-recursively", &format!("{CUSTOM_SCHEMA}:{path}")]);
    }
    let _ = Command::new("systemctl").args(["--user", "disable", "--now", &format!("{TIMER}.timer")]).status();
    for file in [systemd_dir().join(format!("{TIMER}.service")), systemd_dir().join(format!("{TIMER}.timer"))] {
        let _ = std::fs::remove_file(file);
    }
    let _ = Command::new("systemctl").args(["--user", "daemon-reload"]).status();
    let _ = std::fs::remove_file(dirs::data_dir().unwrap_or_default().join(format!("applications/{}.desktop", paths::APP_ID)));
    let _ = Command::new(bin_path()).arg("quit").status();
    let _ = std::fs::remove_file(bin_path());
    println!("Removed the shortcuts, timer, desktop entry and {}.", bin_path().display());
    println!("Your settings ({}) and learning log ({}) were kept.", paths::config_dir().display(), paths::data_dir().display());
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
    fn drill_time_is_validated() {
        assert_eq!(accel_time("09:30"), Some((9, 30)));
        assert_eq!(accel_time("9:30"), None);
        assert_eq!(accel_time("25:00"), None);
    }
}
