//! What the resident instance does without a window: the daily drill reminder and, inside
//! Flatpak, global shortcuts and autostart through portals.

use crate::config::{self, State};
use crate::i18n::{ntrf, tr};
use crate::{learn, paths, srs};
use adw::prelude::*;
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveTime};
use futures_util::StreamExt;
use gtk::{gio, glib};
use std::cell::RefCell;

thread_local! {
    static REMINDER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

pub fn start(app: &adw::Application) {
    reschedule_reminder(app);
    if paths::sandboxed() {
        start_portals(app);
    }
}

/// Seconds from `now` until the next `time` of day.
pub fn seconds_until(time: (u32, u32), now: DateTime<Local>) -> i64 {
    let Some(at) = NaiveTime::from_hms_opt(time.0, time.1, 0) else { return 24 * 3600 };
    let mut next = now.date_naive().and_time(at);
    if next <= now.naive_local() {
        next += ChronoDuration::days(1);
    }
    (next - now.naive_local()).num_seconds().max(1)
}

/// (Re)arm the reminder timer from the current settings. Call after Preferences change.
pub fn reschedule_reminder(app: &adw::Application) {
    REMINDER.with(|slot| {
        if let Some(id) = slot.borrow_mut().take() {
            id.remove();
        }
    });
    let cfg = config::load().unwrap_or_default();
    let Some(time) = config::parse_time(&cfg.reminder_time) else { return };
    if !cfg.reminder {
        return;
    }
    let now = Local::now();
    let due_today = now.time() >= NaiveTime::from_hms_opt(time.0, time.1, 0).unwrap_or_default();
    if due_today {
        notify_if_due(app); // catch up when the computer was off at reminder time
    }
    let app2 = app.clone();
    let seconds = seconds_until(time, now) as u32;
    let id = glib::timeout_add_seconds_local_once(seconds, move || {
        REMINDER.with(|slot| slot.borrow_mut().take());
        notify_if_due(&app2);
        reschedule_reminder(&app2);
    });
    REMINDER.with(|slot| *slot.borrow_mut() = Some(id));
}

fn notify_if_due(app: &adw::Application) {
    let cfg = config::load().unwrap_or_default();
    let today = Local::now().date_naive();
    let mut state = State::load();
    if !cfg.reminder || state.reminded_on == Some(today) {
        return;
    }
    let due = srs::due(&learn::load(), &srs::Deck::load(), today, cfg.drill_size.max(1)).len() as u32;
    if due == 0 {
        return;
    }
    let notification = gio::Notification::new(&tr("Daily English drill"));
    notification.set_body(Some(&ntrf(
        "{} past mistake to review, about {} minute.",
        "{} past mistakes to review, about {} minutes.",
        due,
        &[&due.to_string(), &due.div_ceil(3).max(1).to_string()],
    )));
    notification.set_default_action("app.drill");
    notification.add_button(&tr("Start Drill"), "app.drill");
    app.send_notification(Some("drill"), &notification);
    state.reminded_on = Some(today);
    let _ = state.save();
}

enum PortalEvent {
    Shortcut { id: String, token: Option<String> },
}

/// Global shortcuts and autostart for the Flatpak build, on a portal thread.
fn start_portals(app: &adw::Application) {
    let (tx, rx) = async_channel::unbounded::<PortalEvent>();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
        runtime.block_on(async move {
            if let Err(e) = request_background().await {
                eprintln!("enmaster: background permission: {e}");
            }
            if let Err(e) = global_shortcuts(tx).await {
                eprintln!("enmaster: global shortcuts portal: {e}");
            }
        });
    });
    let app = app.clone();
    glib::spawn_future_local(async move {
        while let Ok(PortalEvent::Shortcut { id, token }) = rx.recv().await {
            super::set_activation_token(token);
            match id.as_str() {
                "coach" => super::coach::open(&app, Some("coach".into())),
                "drill" => super::drill::open(&app),
                _ => super::coach::open(&app, None),
            }
        }
    });
}

/// Ask to keep running after the last window closes and to start at login (for reminders).
async fn request_background() -> ashpd::Result<()> {
    use ashpd::desktop::background::Background;
    let reason = tr("Remind you about daily drills and listen for global shortcuts");
    Background::request()
        .reason(reason.as_str())
        .auto_start(true)
        .command(["enmaster", "--background"])
        .send()
        .await?
        .response()?;
    Ok(())
}

pub const SHORTCUTS: [(&str, &str, &str); 3] = [
    ("coach", "Coach the selected text", "LOGO+ALT+c"),
    ("open", "Open en-Master on the selected text", "LOGO+ALT+e"),
    ("drill", "Daily drill", "LOGO+ALT+d"),
];

async fn global_shortcuts(tx: async_channel::Sender<PortalEvent>) -> ashpd::Result<()> {
    use ashpd::desktop::CreateSessionOptions;
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(CreateSessionOptions::default()).await?;
    let shortcuts: Vec<NewShortcut> = SHORTCUTS
        .iter()
        .map(|(id, description, trigger)| NewShortcut::new(*id, tr(description)).preferred_trigger(*trigger))
        .collect();
    portal.bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default()).await?.response()?;
    let mut activated = portal.receive_activated().await?;
    while let Some(event) = activated.next().await {
        let token = event.options().get("activation_token").and_then(|value| String::try_from(value.clone()).ok());
        if tx.send(PortalEvent::Shortcut { id: event.shortcut_id().to_string(), token }).await.is_err() {
            break;
        }
    }
    drop(session);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::seconds_until;
    use chrono::{Local, TimeZone};

    #[test]
    fn next_reminder_is_today_or_tomorrow() {
        let now = Local.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap();
        assert_eq!(seconds_until((9, 30), now), 30 * 60);
        assert_eq!(seconds_until((8, 0), now), 23 * 3600);
        assert_eq!(seconds_until((9, 0), now), 24 * 3600);
    }
}
