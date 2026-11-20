//! Progress window: what you write, which mistakes you repeat, and the phrases you picked up.
//! Alt+1/2/3 switch the period, Ctrl+T starts the drill, Ctrl+S exports for Anki, Esc closes.

use super::{correction_markup, esc, win_action};
use crate::learn::{self, Report};
use crate::{actions, config, http, srs};
use adw::prelude::*;
use gtk::gio;

const WINDOW_NAME: &str = "progress";
const PERIODS: [(i64, &str); 3] = [(7, "Last 7 days"), (30, "Last 30 days"), (90, "Last 90 days")];

pub fn open(app: &adw::Application) {
    if super::present_existing(app, WINDOW_NAME) {
        return;
    }
    let view = adw::ToolbarView::new();
    let period = adw::ToggleGroup::new();
    for (i, (_, label)) in PERIODS.iter().enumerate() {
        period.add(adw::Toggle::builder().label(*label).name(i.to_string()).tooltip(format!("Alt+{}", i + 1)).build());
    }
    let header = adw::HeaderBar::builder().title_widget(&period).build();
    view.add_top_bar(&header);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&view));
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Progress")
        .default_width(640)
        .default_height(740)
        .content(&toasts)
        .build();
    win.set_widget_name(WINDOW_NAME);

    let refresh = {
        let view = view.clone();
        move |index: usize| {
            let days = PERIODS[index.min(PERIODS.len() - 1)].0;
            let entries = learn::load();
            let report = learn::report(&entries, days, chrono::Local::now());
            view.set_content(Some(&page(&report, &entries)));
        }
    };
    refresh(0);
    period.set_active(0);
    period.connect_active_notify(move |group| refresh(group.active() as usize));
    for i in 0..PERIODS.len() {
        let group = period.clone();
        win_action(&win, &format!("pick-{}", i + 1), move || group.set_active(i as u32));
    }
    let closer = win.clone();
    win_action(&win, "escape", move || closer.close());
    let (win2, toasts2) = (win.clone(), toasts.clone());
    win_action(&win, "export", move || export_anki(&win2, &toasts2));
    win.present();
    super::maybe_snapshot(&win);
}

fn value_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).build();
    row.add_suffix(&gtk::Label::builder().label(value).css_classes(["numeric"]).build());
    row
}

fn rate(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.1}")).unwrap_or_else(|| "–".into())
}

/// An activatable row that runs `action` and shows its shortcut.
fn action_row(title: &str, subtitle: &str, action: &str, accel: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .action_name(action)
        .build();
    row.add_suffix(&super::keycap(accel));
    row
}

fn page(r: &Report, entries: &[learn::Entry]) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let summary = adw::PreferencesGroup::builder().title("Summary").build();
    summary.add(&value_row("Texts checked", &format!("{} (previous {})", r.sessions, r.previous_sessions)));
    summary.add(&value_row("Words written", &r.words.to_string()));
    let trend = match (r.per_100_words(), r.previous_per_100_words()) {
        (Some(now), Some(before)) if now < before => " ↓ better",
        (Some(now), Some(before)) if now > before => " ↑",
        _ => "",
    };
    summary.add(&value_row(
        "Mistakes per 100 words",
        &format!("{} (previous {}){trend}", rate(r.per_100_words()), rate(r.previous_per_100_words())),
    ));
    page.add(&summary);

    let practice = adw::PreferencesGroup::builder().title("Practice").build();
    let cfg = config::load().unwrap_or_default();
    let due = srs::due(entries, &srs::Deck::load(), chrono::Local::now().date_naive(), cfg.drill_size.max(1)).len();
    practice.add(&action_row(
        "Start today's drill",
        &format!("{due} card{} due", if due == 1 { "" } else { "s" }),
        "app.drill",
        "<Control>t",
    ));
    practice.add(&action_row("Export for Anki…", "Your mistakes and phrases as flashcards", "win.export", "<Control>s"));
    let usage = adw::ActionRow::builder()
        .title("Manage usage")
        .subtitle("ChatGPT plan limits for this app")
        .activatable(true)
        .build();
    usage.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
    usage.connect_activated(|_| super::open_uri(http::MANAGE_USAGE_URL));
    practice.add(&usage);
    page.add(&practice);

    if !r.categories.is_empty() {
        let group = adw::PreferencesGroup::builder()
            .title("Mistake types")
            .description("Focus on the top one this week.")
            .build();
        let max = r.categories.iter().map(|c| c.count).max().unwrap_or(1) as f64;
        for c in &r.categories {
            let row = adw::ActionRow::builder()
                .title(actions::category_label(&c.id))
                .subtitle(format!("previous period: {}", c.previous))
                .build();
            let bar = gtk::LevelBar::builder()
                .min_value(0.0)
                .max_value(max)
                .value(c.count as f64)
                .width_request(120)
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&bar);
            row.add_suffix(&gtk::Label::builder().label(c.count.to_string()).width_chars(3).css_classes(["numeric"]).build());
            group.add(&row);
        }
        page.add(&group);
    }

    if !r.repeated.is_empty() {
        let group = adw::PreferencesGroup::builder().title("Mistakes you repeat").build();
        for m in &r.repeated {
            group.add(
                &adw::ActionRow::builder()
                    .title(correction_markup(&m.original, &m.fix))
                    .subtitle(format!("{}× · {}", m.count, esc(&m.explanation)))
                    .title_selectable(true)
                    .build(),
            );
        }
        page.add(&group);
    }

    if !r.tips.is_empty() {
        let group = adw::PreferencesGroup::builder().title("Phrases to remember").build();
        for t in &r.tips {
            group.add(
                &adw::ActionRow::builder()
                    .title(format!("<b>{}</b>", esc(&t.phrase)))
                    .subtitle(format!("{}\n<i>{}</i>", esc(&t.meaning), esc(&t.example)))
                    .title_selectable(true)
                    .build(),
            );
        }
        page.add(&group);
    }

    if r.sessions == 0 {
        page.set_description("No texts checked in this period yet. Select some text anywhere and press Super+Alt+C.");
    }
    page
}

fn export_anki(win: &adw::ApplicationWindow, toasts: &adw::ToastOverlay) {
    let dialog = gtk::FileDialog::builder()
        .title("Export for Anki")
        .initial_name("enmaster-anki.txt")
        .build();
    let toasts = toasts.clone();
    dialog.save(Some(win), gio::Cancellable::NONE, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        let (text, count) = learn::anki_export(&learn::load());
        let message = match std::fs::write(&path, text) {
            Ok(()) => format!("Saved {count} cards. In Anki: File → Import."),
            Err(e) => format!("Could not save: {e}"),
        };
        toasts.add_toast(adw::Toast::new(&message));
    });
}
