//! GTK4/libadwaita front end. One resident instance: a keyboard shortcut starts it the first
//! time and later presses reach it over D-Bus, so windows open fast and the clipboard stays
//! owned after a window closes. Everything can be driven from the keyboard (F1 lists it all).

mod background;
mod coach;
mod drill;
mod preferences;
mod progress;

use crate::i18n::tr;
use crate::{config, paths};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;

thread_local! {
    /// xdg-activation token from a portal shortcut, used by the next window that opens.
    static ACTIVATION_TOKEN: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn run(args: Vec<String>) -> std::process::ExitCode {
    gio::resources_register_include!("enmaster.gresource").expect("bundled resources");
    let app = adw::Application::builder()
        .application_id(paths::APP_ID)
        .resource_base_path(paths::RESOURCE_PATH)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    app.connect_startup(|app| {
        std::mem::forget(app.hold());
        gtk::Window::set_default_icon_name(paths::APP_ID);
        if let Some(display) = gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path(&format!("{}/icons", paths::RESOURCE_PATH));
            let provider = gtk::CssProvider::new();
            provider.load_from_resource(&format!("{}/style.css", paths::RESOURCE_PATH));
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        install_app_actions(app);
        background::start(app);
    });
    // Launched from the app grid (D-Bus activation).
    app.connect_activate(|app| coach::open(app, None));
    app.connect_command_line(|app, cmdline| {
        let args: Vec<String> = cmdline.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        handle(app, &args);
        glib::ExitCode::SUCCESS
    });
    let code = app.run_with_args(&args);
    if code == glib::ExitCode::SUCCESS { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

fn handle(app: &adw::Application, args: &[String]) {
    match args.first().map(String::as_str) {
        Some("quit") => app.quit(),
        Some("--background") => {}
        Some("drill") => drill::open(app),
        Some("progress") => progress::open(app),
        Some("--action") => coach::open(app, args.get(1).cloned()),
        Some(other) if other.starts_with("--action=") => coach::open(app, Some(other["--action=".len()..].to_string())),
        _ => coach::open(app, None),
    }
}

/// Remember the activation token of a portal shortcut for the window it opens.
pub fn set_activation_token(token: Option<String>) {
    ACTIVATION_TOKEN.with(|slot| *slot.borrow_mut() = token);
}

/// Present a new window, using a pending activation token so it gets focus.
pub fn present(win: &impl IsA<gtk::Window>) {
    // Debug builds: ENMASTER_SNAPSHOT_SIZE=360x640 to check narrow layouts.
    #[cfg(debug_assertions)]
    if let Some((w, h)) = std::env::var("ENMASTER_SNAPSHOT_SIZE").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    }) {
        win.set_default_size(w, h);
    }
    if let Some(token) = ACTIVATION_TOKEN.with(|slot| slot.borrow_mut().take()) {
        win.set_startup_id(&token);
    }
    win.present();
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
    add("preferences", &["<Control>comma"], preferences::open);
    add("edit-actions", &[], |_| {
        let _ = config::load(); // creates the commented template on first use
        open_uri(&gio::File::for_path(config::config_path()).uri());
    });
    add("shortcuts", &["<Control>question", "F1"], show_shortcuts);
    add("about", &[], show_about);
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

fn parent_window(app: &adw::Application) -> Option<gtk::Window> {
    app.active_window().or_else(|| app.windows().into_iter().next())
}

fn show_about(app: &adw::Application) {
    let about = adw::AboutDialog::builder()
        .application_name(paths::APP_NAME)
        .application_icon(paths::APP_ID)
        .developer_name("Tecix")
        .developers(["Tecix https://github.com/tecix"])
        .version(paths::VERSION)
        .website(paths::WEBSITE)
        .issue_url(paths::ISSUES)
        .license_type(gtk::License::Gpl30)
        .copyright("© 2026 Tecix")
        .comments(tr("Practise English writing on the text you write"))
        // Translators: put your name and email here, one translator per line.
        .translator_credits(tr("translator-credits"))
        .release_notes("<p>First release.</p>")
        .build();
    about.add_acknowledgement_section(Some(&tr("Inspired by")), &["Plyph https://github.com/ubaimutl/Plyph"]);
    about.present(parent_window(app).as_ref());
}

fn show_shortcuts(app: &adw::Application) {
    let dialog = adw::ShortcutsDialog::new();
    let section = |title: String, items: &[(String, &str)]| {
        let section = adw::ShortcutsSection::new(Some(&title));
        for (label, accel) in items {
            section.add(adw::ShortcutsItem::new(label, accel));
        }
        dialog.add(section);
    };
    section(
        tr("Anywhere in GNOME"),
        &[
            (tr("Coach the selected text"), "<Super><Alt>c"),
            (tr("Open en-Master on the selected text"), "<Super><Alt>e"),
            (tr("Daily drill"), "<Super><Alt>d"),
        ],
    );
    section(
        tr("Writing"),
        &[
            (tr("Run action 1 to 9"), "<Alt>1...<Alt>9"),
            (tr("Pick an action with the arrow keys"), "<Control>k"),
            (tr("Run Coach, or replace once there is a result"), "<Control>Return"),
            (tr("Run the last action again"), "<Control>r"),
            (tr("Edit your text"), "<Control>e"),
            (tr("Switch between result and changes"), "<Control>d"),
            (tr("Copy the result"), "<Control><Shift>c"),
            (tr("Cancel the request, or close"), "Escape"),
        ],
    );
    section(
        tr("Practice"),
        &[
            (tr("Check the answer, or go to the next card"), "Return"),
            (tr("I was right (after a miss)"), "<Alt>r"),
            (tr("Progress"), "<Control>p"),
            (tr("Progress: last 7, 30 or 90 days"), "<Alt>1...<Alt>3"),
            (tr("Progress: export for Anki"), "<Control>s"),
            (tr("Daily drill"), "<Control>t"),
        ],
    );
    section(
        tr("General"),
        &[
            (tr("Keyboard shortcuts"), "<Control>question F1"),
            (tr("Preferences"), "<Control>comma"),
            (tr("Close window"), "<Control>w"),
            (tr("Quit en-Master"), "<Control>q"),
        ],
    );
    dialog.present(parent_window(app).as_ref());
}

/// Bring an already open window of this kind to the front instead of opening another.
pub fn present_existing(app: &adw::Application, name: &str) -> bool {
    match app.windows().into_iter().find(|w| w.widget_name() == name) {
        Some(window) => {
            present(&window);
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
        for (name, label) in
            [("control", "Ctrl"), ("primary", "Ctrl"), ("alt", "Alt"), ("shift", "Shift"), ("super", "Super")]
        {
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

/// One keycap showing a whole shortcut. Screen readers get the shortcut as text.
pub fn keycap(accel: &str) -> gtk::Label {
    gtk::Label::builder().label(key_text(accel)).valign(gtk::Align::Center).css_classes(["keycap"]).build()
}

/// Label plus its keyboard shortcut, for buttons. Returns the keycap so narrow layouts can hide it.
pub fn with_keys(label: &str, accel: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::builder().spacing(8).build();
    row.append(&gtk::Label::builder().label(label).use_underline(true).build());
    let key = keycap(accel);
    row.append(&key);
    (row, key)
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
        flow.append(&gtk::FlowBoxChild::builder().child(&item).focusable(false).build());
        labels.push(label);
    }
    (flow, labels)
}

/// A dim symbolic icon for the start of a list row.
pub fn row_icon(name: &str) -> gtk::Image {
    gtk::Image::builder().icon_name(name).css_classes(["dim-label"]).build()
}

pub fn open_uri(uri: &str) {
    gtk::UriLauncher::new(uri).launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
}

pub fn esc(text: &str) -> String {
    glib::markup_escape_text(text).to_string()
}

/// Error and success colours from the GNOME palette, readable on light and dark backgrounds.
pub fn palette() -> (&'static str, &'static str) {
    if adw::StyleManager::default().is_dark() { ("#ff7b63", "#8ff0a4") } else { ("#c01c28", "#1b8553") }
}

/// `original → fix` with the original struck through.
pub fn correction_markup(original: &str, fix: &str) -> String {
    let (red, green) = palette();
    format!(
        "<span strikethrough=\"true\" foreground=\"{red}\">{}</span>  →  <span foreground=\"{green}\" weight=\"bold\">{}</span>",
        esc(original),
        esc(fix)
    )
}

/// Debug builds only: with `ENMASTER_SNAPSHOT=file.png`, render the window to a PNG and quit.
/// `ENMASTER_SNAPSHOT_ACTION=app.shortcuts` (etc.) runs an action first. Used to check layouts
/// and take the store screenshots in a headless GNOME Shell.
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
            if let Some(renderer) = win.renderer() {
                let snapshot = gtk::Snapshot::new();
                gtk::WidgetPaintable::new(Some(&win)).snapshot(&snapshot, win.width() as f64, win.height() as f64);
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
