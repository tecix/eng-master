//! Daily drill: rewrite your own past mistakes, scheduled with spaced repetition.
//! Enter checks and moves on, Alt+R accepts your answer after a miss, Esc closes.

use super::{correction_markup, esc, win_action, with_keys};
use crate::srs::{Card, Deck};
use crate::{actions, config, learn, srs};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

const WINDOW_NAME: &str = "drill";

struct Drill {
    win: adw::ApplicationWindow,
    view: adw::ToolbarView,
    queue: RefCell<VecDeque<Card>>,
    deck: RefCell<Deck>,
    total: usize,
    answered: Cell<usize>,
    right_first_time: Cell<usize>,
    retried: RefCell<Vec<String>>,
}

pub fn open(app: &adw::Application) {
    if super::present_existing(app, WINDOW_NAME) {
        return;
    }
    let cfg = config::load().unwrap_or_default();
    let deck = Deck::load();
    let today = chrono::Local::now().date_naive();
    let cards = srs::due(&learn::load(), &deck, today, cfg.drill_size.max(1));

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    let win = adw::ApplicationWindow::builder()
        .application(app)
        .title("Daily drill")
        .default_width(640)
        .default_height(540)
        .content(&view)
        .build();
    win.set_widget_name(WINDOW_NAME);
    let closer = win.clone();
    win_action(&win, "escape", move || closer.close());

    let drill = Rc::new(Drill {
        win,
        view,
        total: cards.len(),
        queue: RefCell::new(cards.into()),
        deck: RefCell::new(deck),
        answered: Cell::new(0),
        right_first_time: Cell::new(0),
        retried: RefCell::new(Vec::new()),
    });
    if drill.total == 0 {
        drill.end_page(
            "Nothing to review today",
            "Keep writing with the coach — the mistakes it finds become drill cards.",
        );
    } else {
        drill.next();
    }
    drill.win.present();
    super::maybe_snapshot(&drill.win);
}

/// The sentence with the wrong fragment underlined.
fn prompt_markup(card: &Card) -> String {
    let prompt = card.prompt();
    match prompt.find(&card.mistake.original) {
        Some(start) if !card.mistake.original.is_empty() => {
            let end = start + card.mistake.original.len();
            format!(
                "{}<span underline=\"error\" weight=\"bold\">{}</span>{}",
                esc(&prompt[..start]),
                esc(&prompt[start..end]),
                esc(&prompt[end..])
            )
        }
        _ => esc(prompt),
    }
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(classes.to_vec()).build()
}

impl Drill {
    fn next(self: &Rc<Self>) {
        let Some(card) = self.queue.borrow_mut().pop_front() else {
            let right = self.right_first_time.get();
            self.end_page(
                "Drill done",
                &format!("{right} of {} right on the first try. Cards you missed come back tomorrow.", self.total),
            );
            return;
        };
        let position = (self.answered.get() + 1).min(self.total);
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(14)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(24)
            .margin_end(24)
            .build();
        let progress = gtk::ProgressBar::builder().fraction((position - 1) as f64 / self.total as f64).build();
        column.append(&progress);
        column.append(&label(
            &format!("Card {position} of {} · {}", self.total, actions::category_label(&card.mistake.category)),
            &["dim-label", "caption"],
        ));
        let prompt = label("", &["title-3"]);
        prompt.set_markup(&prompt_markup(&card));
        prompt.set_selectable(true);
        column.append(&prompt);
        column.append(&label("Type the underlined part correctly — or the whole sentence.", &["dim-label"]));
        let entry = gtk::Entry::builder().placeholder_text("Your correction").build();
        column.append(&entry);
        let keys = gtk::Box::builder().spacing(18).build();
        keys.append(&super::hint("Return", "check"));
        keys.append(&super::hint("Escape", "close"));
        column.append(&keys);
        let feedback = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).visible(false).build();
        column.append(&feedback);

        let clamp = adw::Clamp::builder().maximum_size(640).child(&column).build();
        self.view.set_content(Some(&gtk::ScrolledWindow::builder().child(&clamp).build()));
        entry.grab_focus();

        let card = Rc::new(card);
        let this = self.clone();
        let keys2 = keys.clone();
        entry.connect_activate(move |entry| {
            if feedback.is_visible() {
                return;
            }
            let correct = card.check(&entry.text());
            entry.set_editable(false);
            keys2.set_visible(false);
            this.show_feedback(&feedback, card.clone(), correct);
        });
    }

    fn show_feedback(self: &Rc<Self>, feedback: &gtk::Box, card: Rc<Card>, correct: bool) {
        let verdict = if correct { "✓ Correct" } else { "✗ Not quite" };
        feedback.append(&label(verdict, &[if correct { "success" } else { "error" }, "heading"]));
        let fix = label("", &[]);
        fix.set_markup(&correction_markup(&card.mistake.original, &card.mistake.fix));
        fix.set_selectable(true);
        feedback.append(&fix);
        if let Some(sentence) = card.corrected_sentence() {
            feedback.append(&label(&sentence, &[]));
        }
        if !card.mistake.explanation.is_empty() {
            feedback.append(&label(&card.mistake.explanation, &["dim-label"]));
        }
        let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(6).build();
        if !correct {
            // Alt+R through the mnemonic.
            let accept = gtk::Button::builder().child(&with_keys("I was _right", "<Alt>r")).build();
            let this = self.clone();
            let card2 = card.clone();
            accept.connect_clicked(move |_| this.grade(&card2, true));
            buttons.append(&accept);
        }
        let next = gtk::Button::builder()
            .child(&with_keys("Next", "Return"))
            .css_classes(["suggested-action"])
            .build();
        let this = self.clone();
        next.connect_clicked(move |_| this.grade(&card, correct));
        buttons.append(&next);
        feedback.append(&buttons);
        feedback.set_visible(true);
        next.grab_focus();
    }

    fn grade(self: &Rc<Self>, card: &Card, correct: bool) {
        let retry = self.retried.borrow().contains(&card.key);
        if !retry {
            // Only the first attempt moves the card between boxes.
            let today = chrono::Local::now().date_naive();
            let mut deck = self.deck.borrow_mut();
            deck.grade(&card.key, correct, today);
            let _ = deck.save();
            self.answered.set(self.answered.get() + 1);
            if correct {
                self.right_first_time.set(self.right_first_time.get() + 1);
            }
        }
        if !correct && !retry {
            // See it once more at the end of today's session.
            self.retried.borrow_mut().push(card.key.clone());
            self.queue.borrow_mut().push_back(card.clone());
        }
        self.next();
    }

    fn end_page(&self, title: &str, description: &str) {
        let page = adw::StatusPage::builder()
            .icon_name("emblem-ok-symbolic")
            .title(title)
            .description(description)
            .build();
        let buttons = gtk::Box::builder().spacing(12).halign(gtk::Align::Center).build();
        buttons.append(
            &gtk::Button::builder()
                .child(&with_keys("Progress", "<Control>p"))
                .action_name("app.progress")
                .css_classes(["pill"])
                .build(),
        );
        let close = gtk::Button::builder()
            .child(&with_keys("Close", "Escape"))
            .action_name("window.close")
            .css_classes(["pill", "suggested-action"])
            .build();
        buttons.append(&close);
        page.set_child(Some(&buttons));
        self.view.set_content(Some(&page));
        close.grab_focus();
    }
}
