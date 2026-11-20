//! GTK4/libadwaita front end. One resident instance: the keyboard shortcut starts it the first
//! time and later presses reach it over D-Bus, so windows open fast and the clipboard stays
//! owned after a window closes. Everything can be driven from the keyboard (F1 lists it all).

mod coach;
mod drill;
mod progress;

use crate::{config, paths};
use adw::prelude::*;
use gtk::{gdk, gio, glib};

const CSS: &str = "
flowbox.actions > flowboxchild.action-chip {
  padding: 7px 12px;
  border-radius: 999px;
  background-color: color-mix(in srgb, currentColor 8%, transparent);
}
flowbox.actions > flowboxchild.action-chip:hover {
  background-color: color-mix(in srgb, currentColor 14%, transparent);
}
flowbox.actions > flowboxchild.action-chip.primary {
  background-color: var(--accent-bg-color);
  color: var(--accent-fg-color);
}
flowbox.actions > flowboxchild.action-chip:focus-visible {
  outline: 2px solid var(--accent-color);
  outline-offset: 1px;
}
.hints { font-size: 0.9em; }
.keycap {
  font-size: 0.8em;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 5px;
  border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
  box-shadow: inset 0 -1px color-mix(in srgb, currentColor 20%, transparent);
}
.action-chip .keycap { opacity: 0.7; }
.suggested-action .keycap { border-color: color-mix(in srgb, currentColor 45%, transparent); }
.source-chip {
  font-size: 0.85em;
  padding: 1px 8px;
  border-radius: 999px;
  background-color: color-mix(in srgb, currentColor 8%, transparent);
}
";

pub fn run(args: Vec<String>) -> std::process::ExitCode {
    let app = adw::Application::builder()
        .application_id(paths::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(|app| {
        std::mem::forget(app.hold());
        load_css();
        install_app_actions(app);
    });
    app.connect_command_line(|app, cmdline| {
        let args: Vec<String> =
            cmdline.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        handle(app, &args);
        glib::ExitCode::SUCCESS
    });
    let code = app.run_with_args(&args);
    if code == glib::ExitCode::SUCCESS { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

fn handle(app: &adw::Application, args: &[String]) {
    match args.first().map(String::as_str) {
        Some("quit") => app.quit(),
        Some("drill") => drill::open(app),
        Some("progress") => progress::open(app),
        Some("--action") => coach::open(app, args.get(1).cloned()),
        Some(other) if other.starts_with("--action=") => coach::open(app, Some(other["--action=".len()..].to_string())),
        _ => coach::open(app, None),
    }
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

fn install_app_actions(app: &adw::Application) {
    let add = |name: &str, accels: &[&str], f: fn(&adw::Application)| {
        let action = gio::SimpleAction::new(name, None);
        let app_ref = app.clone();
        action.connect_activate(move |_, _| f(&app_ref));
        app.add_action(&action);
        app.set_accels_for_action(&format!("app.{name}"), accels);
    };
    add("progress", &["<Control>p"], progress::open);
    add("drill", &["<Control>t"], drill::open);
    add("settings", &["<Control>comma"], |_| {
        let _ = config::load(); // creates the commented template on first use
        open_uri(&gio::File::for_path(config::config_path()).uri());
    });
    add("shortcuts", &["<Control>question", "F1"], show_shortcuts);
    add("quit", &["<Control>q"], |app| app.quit());

    // Window actions; each window defines the ones it supports.
    let accels: &[(&str, &[&str])] = &[
        ("window.close", &["<Control>w"]),
        ("win.escape", &["Escape"]),
        ("win.primary", &["<Control>Return", "<Control>KP_Enter"]),
        ("win.copy", &["<Control><Shift>c"]),
        ("win.retry", &["<Control>r"]),
        ("win.toggle-view", &["<Control>d"]),
        ("win.focus-text", &["<Control>e"]),
        ("win.focus-actions", &["<Control>k"]),
        ("win.export", &["<Control>s"]),
    ];
    for (action, keys) in accels {
        app.set_accels_for_action(action, keys);
    }
    for n in 1..=9 {
        app.set_accels_for_action(&format!("win.pick-{n}"), &[&format!("<Alt>{n}")]);
    }
}

fn show_shortcuts(app: &adw::Application) {
    let dialog = adw::ShortcutsDialog::new();
    let section = |title: &str, items: &[(&str, &str)]| {
        let section = adw::ShortcutsSection::new(Some(title));
        for (label, accel) in items {
            section.add(adw::ShortcutsItem::new(label, accel));
        }
        dialog.add(section);
    };
    section(
        "Anywhere in GNOME",
        &[
            ("Coach the selected text", "<Super><Alt>c"),
            ("Open en-Master on the selected text", "<Super><Alt>e"),
            ("Daily drill", "<Super><Alt>d"),
        ],
    );
    section(
        "Writing",
        &[
            ("Run action 1 to 9", "<Alt>1...<Alt>9"),
            ("Pick an action with the arrow keys", "<Control>k"),
            ("Run Coach, or Replace once there is a result", "<Control>Return"),
            ("Run the last action again", "<Control>r"),
            ("Edit your text", "<Control>e"),
            ("Switch Result / Changes", "<Control>d"),
            ("Copy the result", "<Control><Shift>c"),
            ("Cancel the request, or close", "Escape"),
        ],
    );
    section(
        "Practice",
        &[
            ("Check the answer / next card", "Return"),
            ("I was right (after a miss)", "<Alt>r"),
            ("Progress", "<Control>p"),
            ("Progress: last 7 / 30 / 90 days", "<Alt>1...<Alt>3"),
            ("Progress: export for Anki", "<Control>s"),
            ("Daily drill", "<Control>t"),
        ],
    );
    section(
        "General",
        &[
            ("Keyboard shortcuts", "<Control>question F1"),
            ("Settings file", "<Control>comma"),
            ("Close window", "<Control>w"),
            ("Quit en-Master", "<Control>q"),
        ],
    );
    let parent = app.active_window().or_else(|| app.windows().into_iter().next());
    dialog.present(parent.as_ref());
}

/// Bring an already open window of this kind to the front instead of opening another.
pub fn present_existing(app: &adw::Application, name: &str) -> bool {
    match app.windows().into_iter().find(|w| w.widget_name() == name) {
        Some(window) => {
            window.present();
            true
        }
        None => false,
    }
}

/// A window action that runs `f`.
pub fn win_action(win: &impl IsA<gio::ActionMap>, name: &str, f: impl Fn() + 'static) -> gio::SimpleAction {
    let action = gio::SimpleAction::new(name, None);
    action.connect_activate(move |_, _| f());
    win.add_action(&action);
    action
}

/// Human-readable form of a GTK accelerator: `<Control><Shift>c` → `Ctrl+Shift+C`.
/// Space-separated accelerators become alternatives, `a...b` a range.
pub fn key_text(accel: &str) -> String {
    let one = |accel: &str| {
        let mut mods = Vec::new();
        let mut rest = accel;
        while let Some(tail) = rest.strip_prefix('<') {
            let (name, after) = tail.split_once('>').unwrap_or((tail, ""));
            mods.push(name.to_ascii_lowercase());
            rest = after;
        }
        let mut parts: Vec<String> = Vec::new();
        for (name, label) in [("control", "Ctrl"), ("primary", "Ctrl"), ("alt", "Alt"), ("shift", "Shift"), ("super", "Super")] {
            if mods.iter().any(|m| m == name) && !parts.iter().any(|p| p == label) {
                parts.push(label.to_string());
            }
        }
        parts.push(match rest {
            "Return" | "KP_Enter" => "Enter".to_string(),
            "Escape" => "Esc".to_string(),
            "question" => "?".to_string(),
            "comma" => ",".to_string(),
            key if key.chars().count() == 1 => key.to_uppercase(),
            key => key.to_string(),
        });
        parts.join("+")
    };
    accel
        .split_whitespace()
        .map(|alt| match alt.split_once("...") {
            Some((from, to)) => format!("{}–{}", one(from), one(to).rsplit('+').next().unwrap_or_default()),
            None => one(alt),
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// One keycap showing a whole shortcut.
pub fn keycap(accel: &str) -> gtk::Label {
    gtk::Label::builder().label(key_text(accel)).valign(gtk::Align::Center).css_classes(["keycap"]).build()
}

/// Label plus its keyboard shortcut, for buttons.
pub fn with_keys(label: &str, accel: &str) -> gtk::Box {
    let row = gtk::Box::builder().spacing(8).build();
    row.append(&gtk::Label::builder().label(label).use_underline(true).build());
    row.append(&keycap(accel));
    row
}

/// One dim `key description` hint.
pub fn hint(accel: &str, text: &str) -> gtk::Box {
    let item = gtk::Box::builder().spacing(6).valign(gtk::Align::Center).css_classes(["hints", "dim-label"]).build();
    item.append(&keycap(accel));
    item.append(&gtk::Label::new(Some(text)));
    item
}

/// Dim, wrapping `key description` hints; returns the description labels so they can change.
pub fn hints(items: &[(&str, &str)]) -> (gtk::FlowBox, Vec<gtk::Label>) {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(14)
        .row_spacing(4)
        .max_children_per_line(12)
        .can_focus(false)
        .css_classes(["hints", "dim-label"])
        .build();
    let mut labels = Vec::new();
    for (accel, text) in items {
        let item = gtk::Box::builder().spacing(6).build();
        item.append(&keycap(accel));
        let label = gtk::Label::new(Some(text));
        item.append(&label);
        let child = gtk::FlowBoxChild::builder().child(&item).focusable(false).build();
        flow.append(&child);
        labels.push(label);
    }
    (flow, labels)
}

pub fn open_uri(uri: &str) {
    gtk::UriLauncher::new(uri).launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
}

pub fn esc(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

pub const RED: &str = "#e01b24";
pub const GREEN: &str = "#26a269";

/// `original → fix` with the original struck through.
pub fn correction_markup(original: &str, fix: &str) -> String {
    format!(
        "<span strikethrough=\"true\" foreground=\"{RED}\">{}</span>  →  <span foreground=\"{GREEN}\" weight=\"bold\">{}</span>",
        esc(original),
        esc(fix)
    )
}

/// Debug builds only: with `ENMASTER_SNAPSHOT=file.png`, render the window to a PNG and quit.
/// `ENMASTER_SNAPSHOT_ACTION=app.shortcuts` (etc.) runs an action first. Used to check layouts
/// headlessly with the Broadway backend.
#[cfg(debug_assertions)]
pub fn maybe_snapshot(win: &impl IsA<gtk::Window>) {
    let Some(path) = std::env::var_os("ENMASTER_SNAPSHOT") else { return };
    // Broadway without a browser never advances animations, so dialogs would stay invisible.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
    let win = win.clone().upcast::<gtk::Window>();
    glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
        if let Ok(name) = std::env::var("ENMASTER_SNAPSHOT_ACTION") {
            let _ = WidgetExt::activate_action(&win, &name, None);
        }
        glib::timeout_add_local_once(std::time::Duration::from_millis(800), move || {
            // An open dialog (e.g. keyboard shortcuts) is captured on its own.
            fn find_dialog(w: &gtk::Widget) -> Option<gtk::Widget> {
                let mut child = w.first_child();
                while let Some(c) = child {
                    if c.is::<adw::Dialog>() {
                        return Some(c);
                    }
                    if let Some(found) = find_dialog(&c) {
                        return Some(found);
                    }
                    child = c.next_sibling();
                }
                None
            }
            let target = find_dialog(win.upcast_ref()).unwrap_or_else(|| win.clone().upcast());
            if let Some(renderer) = win.renderer() {
                let snapshot = gtk::Snapshot::new();
                gtk::WidgetPaintable::new(Some(&target)).snapshot(&snapshot, target.width() as f64, target.height() as f64);
                if let Some(node) = snapshot.to_node()
                    && let Err(e) = renderer.render_texture(&node, None).save_to_png(&path)
                {
                    eprintln!("snapshot failed: {e}");
                }
            }
            if let Some(app) = win.application() {
                app.quit();
            }
        });
    });
}

#[cfg(not(debug_assertions))]
pub fn maybe_snapshot(_win: &impl IsA<gtk::Window>) {}

#[cfg(test)]
mod tests {
    use super::key_text;

    #[test]
    fn accelerators_read_like_people_type_them() {
        assert_eq!(key_text("<Control><Shift>c"), "Ctrl+Shift+C");
        assert_eq!(key_text("<Control>Return"), "Ctrl+Enter");
        assert_eq!(key_text("Escape"), "Esc");
        assert_eq!(key_text("<Alt>1...<Alt>9"), "Alt+1–9");
        assert_eq!(key_text("<Control>question F1"), "Ctrl+? / F1");
    }
}
