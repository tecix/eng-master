//! Preferences: how the coach writes, the learning log and drills, the ChatGPT account and the
//! keyboard shortcuts. Every change is saved to `config.toml` right away.

use super::{background, keycap, row_icon};
use crate::config::{self, Config};
use crate::i18n::{tr, trf};
use crate::{api, auth, http, install, paths};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

const LEVELS: [&str; 5] =
    ["A2 (elementary)", "B1 (intermediate)", "B1-B2 (intermediate)", "B2 (upper intermediate)", "C1 (advanced)"];
const EFFORTS: [&str; 4] = ["auto", "low", "medium", "high"];

type Shared = Rc<RefCell<Config>>;

fn level_label(level: &str) -> String {
    match level {
        "A2 (elementary)" => tr("A2, elementary"),
        "B1 (intermediate)" => tr("B1, intermediate"),
        "B1-B2 (intermediate)" => tr("B1 to B2, intermediate"),
        "B2 (upper intermediate)" => tr("B2, upper intermediate"),
        "C1 (advanced)" => tr("C1, advanced"),
        other => other.to_string(),
    }
}

/// Save after a change and re-arm the reminder.
fn save(app: &adw::Application, cfg: &Shared) {
    if let Err(e) = config::save(&cfg.borrow()) {
        eprintln!("enmaster: could not save settings: {e:#}");
    }
    background::reschedule_reminder(app);
}

/// A combo row over fixed values with translated labels.
fn combo(title: &str, values: &[String], labels: &[String], current: &str) -> adw::ComboRow {
    let model = gtk::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
    let row = adw::ComboRow::builder().title(title).model(&model).build();
    row.set_selected(values.iter().position(|v| v == current).unwrap_or(0) as u32);
    row
}

pub fn open(app: &adw::Application) {
    let cfg: Shared = Rc::new(RefCell::new(config::load().unwrap_or_default()));
    let dialog = adw::PreferencesDialog::builder().search_enabled(true).build();
    dialog.add(&writing_page(app, &cfg));
    dialog.add(&learning_page(app, &cfg));
    dialog.add(&account_page(&dialog));
    dialog.present(app.active_window().or_else(|| app.windows().into_iter().next()).as_ref());
}

fn writing_page(app: &adw::Application, cfg: &Shared) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("Writing")).icon_name("document-edit-symbolic").build();
    let current = cfg.borrow().clone();

    let coach = adw::PreferencesGroup::builder().title(tr("Coach")).build();

    // Model: "Automatic" plus whatever the plan lists (loaded in the background).
    let model_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![String::new()]));
    let mut labels = vec![tr("Automatic")];
    if !current.model.is_empty() {
        model_ids.borrow_mut().push(current.model.clone());
        labels.push(current.model.clone());
    }
    let model_row = combo(&tr("Model"), &model_ids.borrow(), &labels, &current.model);
    model_row.set_subtitle(&tr("Automatic uses the first model your plan offers"));
    {
        let (app, cfg, ids) = (app.clone(), cfg.clone(), model_ids.clone());
        model_row.connect_selected_notify(move |row| {
            if let Some(id) = ids.borrow().get(row.selected() as usize) {
                cfg.borrow_mut().model = id.clone();
                save(&app, &cfg);
            }
        });
    }
    {
        let (row, ids, wanted) = (model_row.clone(), model_ids.clone(), current.model.clone());
        glib::spawn_future_local(async move {
            let listed =
                gio::spawn_blocking(|| auth::access_token().and_then(|t| api::models(auth::RESOURCE, &t))).await;
            let Ok(Ok(models)) = listed else { return };
            let mut new_ids = vec![String::new()];
            let mut new_labels = vec![tr("Automatic")];
            for m in models {
                new_ids.push(m.id.clone());
                new_labels.push(m.name);
            }
            if !wanted.is_empty() && !new_ids.contains(&wanted) {
                new_ids.push(wanted.clone());
                new_labels.push(wanted.clone());
            }
            let selected = new_ids.iter().position(|id| *id == wanted).unwrap_or(0) as u32;
            // Swap the ids first: the selection handler reads them.
            *ids.borrow_mut() = new_ids;
            row.set_model(Some(&gtk::StringList::new(&new_labels.iter().map(String::as_str).collect::<Vec<_>>())));
            row.set_selected(selected);
        });
    }
    coach.add(&model_row);

    let effort_values: Vec<String> = EFFORTS.iter().map(|s| s.to_string()).collect();
    let effort_labels = vec![tr("Automatic"), tr("Low (fastest)"), tr("Medium"), tr("High")];
    let effort = combo(&tr("Thinking Effort"), &effort_values, &effort_labels, &current.reasoning_effort);
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        effort.connect_selected_notify(move |row| {
            cfg.borrow_mut().reasoning_effort = EFFORTS[(row.selected() as usize).min(EFFORTS.len() - 1)].into();
            save(&app, &cfg);
        });
    }
    coach.add(&effort);

    let mut level_values: Vec<String> = LEVELS.iter().map(|s| s.to_string()).collect();
    if !level_values.contains(&current.level) {
        level_values.push(current.level.clone());
    }
    let level_labels: Vec<String> = level_values.iter().map(|l| level_label(l)).collect();
    let level = combo(&tr("Your English Level"), &level_values, &level_labels, &current.level);
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        level.connect_selected_notify(move |row| {
            if let Some(value) = level_values.get(row.selected() as usize) {
                cfg.borrow_mut().level = value.clone();
                save(&app, &cfg);
            }
        });
    }
    coach.add(&level);

    let language = adw::EntryRow::builder()
        .title(tr("Explain Mistakes In"))
        .text(&current.explain_language)
        .show_apply_button(true)
        .build();
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        language.connect_apply(move |row| {
            cfg.borrow_mut().explain_language = row.text().trim().to_string();
            save(&app, &cfg);
        });
    }
    coach.add(&language);

    let about_you = adw::EntryRow::builder()
        .title(tr("About You and Who You Write To"))
        .text(&current.personal_context)
        .show_apply_button(true)
        .build();
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        about_you.connect_apply(move |row| {
            cfg.borrow_mut().personal_context = row.text().trim().to_string();
            save(&app, &cfg);
        });
    }
    coach.add(&about_you);
    page.add(&coach);

    if !paths::sandboxed() {
        let replace = adw::PreferencesGroup::builder().title(tr("Replace")).build();
        let paste = adw::SwitchRow::builder()
            .title(tr("Paste Automatically"))
            .subtitle(tr("Press Ctrl+V in the app you came from, using ydotool"))
            .active(!current.paste_command.is_empty())
            .build();
        let (app, cfg) = (app.clone(), cfg.clone());
        paste.connect_active_notify(move |row| {
            cfg.borrow_mut().paste_command = if row.is_active() { Config::default().paste_command } else { Vec::new() };
            save(&app, &cfg);
        });
        replace.add(&paste);
        page.add(&replace);
    }

    let custom = adw::PreferencesGroup::builder()
        .title(tr("Custom Actions"))
        .description(tr("Add your own actions, such as a polite reminder or an apology, in the settings file."))
        .build();
    let edit = adw::ActionRow::builder()
        .title(tr("Edit Custom Actions"))
        .activatable(true)
        .action_name("app.edit-actions")
        .build();
    edit.add_suffix(&row_icon("adw-external-link-symbolic"));
    custom.add(&edit);
    page.add(&custom);
    page
}

fn reminder_times(current: &str) -> Vec<String> {
    let mut times: Vec<String> = (12..46).map(|half| format!("{:02}:{:02}", half / 2, (half % 2) * 30)).collect();
    if config::parse_time(current).is_some() && !times.iter().any(|t| t == current) {
        times.push(current.to_string());
        times.sort();
    }
    times
}

fn learning_page(app: &adw::Application, cfg: &Shared) -> adw::PreferencesPage {
    let page =
        adw::PreferencesPage::builder().title(tr("Learning")).icon_name("media-playlist-repeat-symbolic").build();
    let current = cfg.borrow().clone();

    let log = adw::PreferencesGroup::builder().title(tr("Learning Log")).build();
    let full_text = adw::SwitchRow::builder()
        .title(tr("Keep Full Texts"))
        .subtitle(tr("Store what you wrote and the result, not only the mistakes"))
        .active(current.log_full_text)
        .build();
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        full_text.connect_active_notify(move |row| {
            cfg.borrow_mut().log_full_text = row.is_active();
            save(&app, &cfg);
        });
    }
    log.add(&full_text);
    page.add(&log);

    let drill = adw::PreferencesGroup::builder().title(tr("Daily Drill")).build();
    let cards = adw::SpinRow::with_range(3.0, 30.0, 1.0);
    cards.set_title(&tr("Cards per Drill"));
    cards.set_value(current.drill_size as f64);
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        cards.connect_value_notify(move |row| {
            cfg.borrow_mut().drill_size = row.value() as usize;
            save(&app, &cfg);
        });
    }
    drill.add(&cards);

    let reminder = adw::SwitchRow::builder()
        .title(tr("Daily Reminder"))
        .subtitle(tr("A notification when you have mistakes to review"))
        .active(current.reminder)
        .build();
    let times = reminder_times(&current.reminder_time);
    let time = combo(&tr("Reminder Time"), &times, &times, &current.reminder_time);
    reminder.bind_property("active", &time, "sensitive").sync_create().build();
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        reminder.connect_active_notify(move |row| {
            cfg.borrow_mut().reminder = row.is_active();
            save(&app, &cfg);
        });
    }
    {
        let (app, cfg) = (app.clone(), cfg.clone());
        time.connect_selected_notify(move |row| {
            if let Some(value) = times.get(row.selected() as usize) {
                cfg.borrow_mut().reminder_time = value.clone();
                save(&app, &cfg);
            }
        });
    }
    drill.add(&reminder);
    drill.add(&time);
    page.add(&drill);
    page
}

fn account_page(dialog: &adw::PreferencesDialog) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder().title(tr("Account")).icon_name("avatar-default-symbolic").build();

    let account = adw::PreferencesGroup::builder()
        .title(tr("ChatGPT"))
        .description(tr("Your text is sent to OpenAI only when you run an action."))
        .build();
    let status = adw::ActionRow::builder().title(tr("Checking…")).build();
    status.add_prefix(&row_icon("avatar-default-symbolic"));
    let button = gtk::Button::builder().valign(gtk::Align::Center).visible(false).build();
    status.add_suffix(&button);
    account.add(&status);
    let usage = adw::ActionRow::builder()
        .title(tr("Manage Usage"))
        .subtitle(tr("Your plan’s limits for this app"))
        .activatable(true)
        .build();
    usage.add_suffix(&row_icon("adw-external-link-symbolic"));
    usage.connect_activated(|_| super::open_uri(http::MANAGE_USAGE_URL));
    account.add(&usage);
    page.add(&account);

    let refresh: Rc<dyn Fn()> = {
        let (status, button) = (status.clone(), button.clone());
        Rc::new(move || {
            let (status, button) = (status.clone(), button.clone());
            glib::spawn_future_local(async move {
                let signed_in = match gio::spawn_blocking(auth::status).await {
                    Ok(Ok(auth::Status::SignedIn { email, .. })) => {
                        status.set_title(&if email.is_empty() {
                            tr("Connected")
                        } else {
                            trf("Connected as {}", &[&email])
                        });
                        true
                    }
                    _ => {
                        status.set_title(&tr("Not connected"));
                        false
                    }
                };
                button.set_label(&if signed_in { tr("Sign _Out") } else { tr("_Continue with ChatGPT") });
                button.set_use_underline(true);
                button.set_css_classes(if signed_in { &["destructive-action"] } else { &["suggested-action"] });
                button.set_sensitive(true);
                button.set_visible(true);
            });
        })
    };
    refresh();
    {
        let (refresh, dialog) = (refresh.clone(), dialog.clone());
        button.connect_clicked(move |button| {
            button.set_sensitive(false);
            let signing_in = button.has_css_class("suggested-action");
            let (refresh, dialog) = (refresh.clone(), dialog.clone());
            glib::spawn_future_local(async move {
                if signing_in {
                    let done = gio::spawn_blocking(|| auth::sign_in(&AtomicBool::new(false), false)).await;
                    match done {
                        Ok(Ok(_)) => super::coach::welcome_once(dialog.upcast_ref()),
                        Ok(Err(e)) => dialog.add_toast(adw::Toast::new(&format!("{e:#}"))),
                        Err(_) => {}
                    }
                } else {
                    let _ = gio::spawn_blocking(auth::sign_out).await;
                }
                refresh();
            });
        });
    }

    let shortcuts = adw::PreferencesGroup::builder().title(tr("Keyboard Shortcuts")).build();
    shortcuts.set_description(Some(&if paths::sandboxed() {
        tr("Work in any app. Change them in Settings → Apps → en-Master.")
    } else {
        tr("Work in any app. They are custom shortcuts in Settings → Keyboard.")
    }));
    for (label, accel) in [
        (tr("Coach the selected text"), "<Super><Alt>c"),
        (tr("Open en-Master on the selected text"), "<Super><Alt>e"),
        (tr("Daily drill"), "<Super><Alt>d"),
    ] {
        let row = adw::ActionRow::builder().title(label).build();
        row.add_suffix(&keycap(accel));
        shortcuts.add(&row);
    }
    if !paths::sandboxed() && !install::shortcuts_installed() {
        let add = gtk::Button::builder()
            .label(tr("_Add to GNOME"))
            .use_underline(true)
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .build();
        let dialog = dialog.clone();
        add.connect_clicked(move |button| {
            let message = match install::install_shortcuts_only() {
                Ok(_) => tr("Shortcuts added"),
                Err(e) => format!("{e:#}"),
            };
            button.set_visible(false);
            dialog.add_toast(adw::Toast::new(&message));
        });
        shortcuts.set_header_suffix(Some(&add));
    }
    page.add(&shortcuts);

    page
}
