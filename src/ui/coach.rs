//! The coach window: your text, the actions, the result, the changes and what to learn.
//! Keyboard first: Alt+1…9 run actions, Ctrl+Enter runs Coach or replaces, Esc cancels or closes.

use super::{correction_markup, esc, hints, win_action, with_keys};
use crate::actions::{self, Action};
use crate::config::{self, Config, State};
use crate::engine::{self, Outcome};
use crate::selection::{self, Source};
use crate::{auth, http, paths};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const WINDOW_NAME: &str = "coach";

enum Msg {
    Progress(usize),
    Done(anyhow::Result<Outcome>),
}

pub struct Coach {
    cfg: Config,
    actions: Vec<Action>,
    win: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    pages: gtk::Stack,
    title: adw::WindowTitle,
    source_chip: gtk::Label,
    input: gtk::TextView,
    chips: gtk::FlowBox,
    primary_hint: gtk::Label,
    busy: gtk::Box,
    busy_label: gtk::Label,
    error_label: gtk::Label,
    result_section: gtk::Box,
    result_stack: adw::ViewStack,
    result_view: gtk::TextView,
    diff_view: gtk::TextView,
    learn_list: gtk::ListBox,
    signin_button: gtk::Button,
    signin_label: gtk::Label,
    copy_action: gio::SimpleAction,
    replace_action: gio::SimpleAction,
    original: RefCell<String>,
    last_action: RefCell<Option<Action>>,
    has_result: Cell<bool>,
    /// Id of the request in flight; 0 when idle. Esc resets it so a late answer is ignored.
    request: Cell<u64>,
    next_request: Cell<u64>,
    cancel_signin: Arc<AtomicBool>,
}

pub fn open(app: &adw::Application, action_id: Option<String>) {
    for window in app.windows() {
        if window.widget_name() == WINDOW_NAME {
            window.close();
        }
    }
    let (cfg, cfg_error) = match config::load() {
        Ok(cfg) => (cfg, None),
        Err(e) => (Config::default(), Some(format!("{e:#}"))),
    };
    // Read the selection before our window takes focus.
    let (text, source) = selection::capture();
    let coach = Coach::new(app, cfg);
    coach.input.buffer().set_text(&text);
    coach.source_chip.set_label(source.describe());
    coach.win.present();
    super::maybe_snapshot(&coach.win);
    #[cfg(debug_assertions)]
    if coach.demo() {
        return;
    }
    if source == Source::Empty {
        coach.input.grab_focus();
    } else {
        coach.focus_actions();
    }
    if let Some(error) = cfg_error {
        coach.show_error(&format!("Settings file has an error, using defaults: {error}"));
    }

    let coach2 = coach.clone();
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(auth::status).await {
            Ok(Ok(auth::Status::SignedIn { plan_access: true, .. })) => {
                if let Some(id) = action_id {
                    match coach2.actions.iter().find(|a| a.id == id).cloned() {
                        Some(action) => coach2.run_action(action),
                        None => coach2.show_error(&format!("Unknown action \"{id}\".")),
                    }
                }
            }
            Ok(Ok(_)) => coach2.show_signin(None),
            Ok(Err(e)) => coach2.show_error(&format!("{e:#}")),
            Err(_) => coach2.show_error("Could not check the ChatGPT connection."),
        }
    });
}

fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
}

fn text_view(editable: bool) -> gtk::TextView {
    gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .editable(editable)
        .cursor_visible(editable)
        .accepts_tab(false)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(12)
        .right_margin(12)
        .build()
}

fn framed(view: &gtk::TextView, min_height: i32) -> gtk::Frame {
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(min_height)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(view)
        .build();
    gtk::Frame::builder().child(&scroller).build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).hexpand(true).css_classes(["heading"]).build()
}

impl Coach {
    fn new(app: &adw::Application, cfg: Config) -> Rc<Coach> {
        let actions = actions::all(&cfg);
        let title = adw::WindowTitle::new(paths::APP_NAME, "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Progress"), Some("app.progress"));
        section.append(Some("Daily Drill"), Some("app.drill"));
        menu.append_section(None, &section);
        let section = gio::Menu::new();
        section.append(Some("Settings File"), Some("app.settings"));
        section.append(Some("Keyboard Shortcuts"), Some("app.shortcuts"));
        section.append(Some("Sign Out of ChatGPT"), Some("win.signout"));
        section.append(Some("Quit en-Master"), Some("app.quit"));
        menu.append_section(None, &section);
        header.pack_end(
            &gtk::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .menu_model(&menu)
                .primary(true)
                .tooltip_text("Main Menu (F10)")
                .build(),
        );
        header.pack_end(
            &gtk::Button::builder()
                .icon_name("preferences-desktop-keyboard-shortcuts-symbolic")
                .action_name("app.shortcuts")
                .tooltip_text("Keyboard Shortcuts (F1)")
                .build(),
        );

        // Your text
        let source_chip = gtk::Label::builder().css_classes(["source-chip", "dim-label"]).build();
        let input_head = gtk::Box::builder().spacing(8).build();
        input_head.append(&gtk::Label::builder().label("Your text").xalign(0.0).css_classes(["heading"]).build());
        input_head.append(&source_chip);
        input_head.append(&gtk::Box::builder().hexpand(true).build());
        input_head.append(&super::hint("<Control>e", "edit"));
        let input = text_view(true);

        // Action chips: arrows + Enter, Alt+N, or click.
        let chips = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .column_spacing(8)
            .row_spacing(8)
            .max_children_per_line(8)
            .css_classes(["actions"])
            .build();
        for (i, action) in actions.iter().enumerate() {
            let row = gtk::Box::builder().spacing(8).build();
            row.append(&gtk::Label::new(Some(&action.label)));
            if i < 9 {
                row.append(&super::keycap(&format!("<Alt>{}", i + 1)));
            }
            let chip = gtk::FlowBoxChild::builder().child(&row).css_classes(["action-chip"]).build();
            if i == 0 {
                chip.add_css_class("primary");
            }
            chips.append(&chip);
        }
        let (hint_row, hint_labels) = hints(&[
            ("<Control>Return", "run Coach"),
            ("<Control>k", "pick with arrow keys"),
            ("<Control>r", "run again"),
            ("F1", "all shortcuts"),
        ]);
        let primary_hint = hint_labels[0].clone();

        let busy_label = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["dim-label"]).build();
        let busy = gtk::Box::builder().spacing(8).visible(false).build();
        busy.append(&adw::Spinner::new());
        busy.append(&busy_label);
        busy.append(&super::hint("Escape", "cancel"));
        let error_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .css_classes(["error"])
            .visible(false)
            .build();

        // Result / Changes
        let result_view = text_view(true);
        let diff_view = text_view(false);
        let buffer = diff_view.buffer();
        buffer.create_tag(Some("del"), &[("foreground", &super::RED), ("strikethrough", &true)]);
        buffer.create_tag(Some("ins"), &[("foreground", &super::GREEN), ("weight", &700i32)]);
        let result_stack = adw::ViewStack::new();
        result_stack.add_titled_with_icon(&framed(&result_view, 90), Some("result"), "Result", "document-edit-symbolic");
        result_stack.add_titled_with_icon(&framed(&diff_view, 90), Some("changes"), "Changes", "view-dual-symbolic");
        let switcher_row = gtk::Box::builder().spacing(8).build();
        switcher_row.append(
            &adw::InlineViewSwitcher::builder().stack(&result_stack).hexpand(true).halign(gtk::Align::Start).build(),
        );
        switcher_row.append(&super::hint("<Control>d", "switch"));
        let learn_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let result_section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).visible(false).build();
        result_section.append(&switcher_row);
        result_section.append(&result_stack);
        result_section.append(&heading("What to learn"));
        result_section.append(&learn_list);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        column.append(&input_head);
        column.append(&framed(&input, 90));
        column.append(&chips);
        column.append(&hint_row);
        column.append(&busy);
        column.append(&error_label);
        column.append(&result_section);
        let clamp = adw::Clamp::builder().maximum_size(860).tightening_threshold(700).child(&column).build();
        let editor = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        // Sign-in page
        let signin_button = gtk::Button::builder()
            .label("Continue with ChatGPT")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let signin_label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).css_classes(["dim-label"]).build();
        let signin_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        signin_box.append(&signin_button);
        signin_box.append(&signin_label);
        let signin = adw::StatusPage::builder()
            .icon_name("accessories-dictionary-symbolic")
            .title("Connect your ChatGPT plan")
            .description("en-Master uses your ChatGPT plan — no API key needed.\nSign-in opens in your browser. Press Enter to start.")
            .child(&signin_box)
            .build();

        let pages = gtk::Stack::new();
        pages.add_named(&editor, Some("editor"));
        pages.add_named(&signin, Some("signin"));
        pages.set_visible_child_name("editor");

        // Bottom bar: plan notice (OpenAI UI guidelines) and result buttons with their keys.
        let plan = gtk::Label::builder()
            .use_markup(true)
            .label(format!("Using ChatGPT plan · <a href=\"{}\">Manage usage</a>", http::MANAGE_USAGE_URL))
            .margin_start(6)
            .css_classes(["dim-label", "caption"])
            .build();
        let copy_button = gtk::Button::builder()
            .child(&with_keys("Copy", "<Control><Shift>c"))
            .action_name("win.copy")
            .build();
        let replace_button = gtk::Button::builder()
            .child(&with_keys("Replace", "<Control>Return"))
            .action_name("win.replace")
            .tooltip_text("Paste into the app you came from")
            .css_classes(["suggested-action"])
            .build();
        let bar = gtk::ActionBar::new();
        bar.pack_start(&plan);
        bar.pack_end(&replace_button);
        bar.pack_end(&copy_button);

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&pages));
        view.add_bottom_bar(&bar);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&view));
        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title(paths::APP_NAME)
            .default_width(780)
            .default_height(740)
            .content(&toasts)
            .build();
        win.set_widget_name(WINDOW_NAME);

        let copy_action = gio::SimpleAction::new("copy", None);
        let replace_action = gio::SimpleAction::new("replace", None);
        copy_action.set_enabled(false);
        replace_action.set_enabled(false);
        win.add_action(&copy_action);
        win.add_action(&replace_action);

        let coach = Rc::new(Coach {
            cfg,
            actions,
            win,
            toasts,
            pages,
            title,
            source_chip,
            input,
            chips,
            primary_hint,
            busy,
            busy_label,
            error_label,
            result_section,
            result_stack,
            result_view,
            diff_view,
            learn_list,
            signin_button,
            signin_label,
            copy_action,
            replace_action,
            original: RefCell::new(String::new()),
            last_action: RefCell::new(None),
            has_result: Cell::new(false),
            request: Cell::new(0),
            next_request: Cell::new(0),
            cancel_signin: Arc::new(AtomicBool::new(false)),
        });
        coach.connect();
        coach
    }

    fn connect(self: &Rc<Self>) {
        let this = self.clone();
        self.chips.connect_child_activated(move |_, chip| {
            if let Some(action) = usize::try_from(chip.index()).ok().and_then(|i| this.actions.get(i)) {
                this.run_action(action.clone());
            }
        });
        for (i, action) in self.actions.iter().take(9).enumerate() {
            let this = self.clone();
            let action = action.clone();
            win_action(&self.win, &format!("pick-{}", i + 1), move || this.run_action(action.clone()));
        }

        let this = self.clone();
        self.copy_action.connect_activate(move |_, _| {
            this.win.clipboard().set_text(&text_of(&this.result_view));
            this.toast("Copied");
        });
        let this = self.clone();
        self.replace_action.connect_activate(move |_, _| this.replace());

        let this = self.clone();
        win_action(&self.win, "primary", move || {
            if this.pages.visible_child_name().as_deref() == Some("signin") {
                this.signin_button.emit_clicked();
            } else if this.has_result.get() {
                this.replace();
            } else if let Some(first) = this.actions.first() {
                this.run_action(first.clone());
            }
        });
        let this = self.clone();
        win_action(&self.win, "retry", move || {
            let last = this.last_action.borrow().clone();
            match last {
                Some(action) => this.run_action(action),
                None => this.toast("No action to repeat yet"),
            }
        });
        let this = self.clone();
        win_action(&self.win, "toggle-view", move || {
            if this.has_result.get() {
                let showing_changes = this.result_stack.visible_child_name().as_deref() == Some("changes");
                this.result_stack.set_visible_child_name(if showing_changes { "result" } else { "changes" });
            }
        });
        let this = self.clone();
        win_action(&self.win, "focus-text", move || {
            this.input.grab_focus();
        });
        let this = self.clone();
        win_action(&self.win, "focus-actions", move || this.focus_actions());
        let this = self.clone();
        win_action(&self.win, "escape", move || {
            if this.request.get() != 0 {
                this.cancel();
            } else {
                this.win.close();
            }
        });

        let this = self.clone();
        self.signin_button.connect_clicked(move |_| this.sign_in());
        let this = self.clone();
        win_action(&self.win, "signout", move || {
            let this = this.clone();
            glib::spawn_future_local(async move {
                match gio::spawn_blocking(auth::sign_out).await {
                    Ok(Ok(true)) => this.show_signin(Some("Signed out.")),
                    Ok(Ok(false)) => {
                        this.show_signin(Some("Signed out here. You can also disconnect the app in ChatGPT settings."))
                    }
                    Ok(Err(e)) => this.show_error(&format!("{e:#}")),
                    Err(_) => {}
                }
            });
        });

        let cancel = self.cancel_signin.clone();
        self.win.connect_close_request(move |_| {
            cancel.store(true, Ordering::Relaxed);
            glib::Propagation::Proceed
        });
    }

    /// Debug builds only: `ENMASTER_DEMO=result|signin|busy` shows a page without network.
    #[cfg(debug_assertions)]
    fn demo(&self) -> bool {
        match std::env::var("ENMASTER_DEMO").as_deref() {
            Ok("signin") => self.show_signin(None),
            Ok("result") => {
                let original = "Hi John, I has send the report yesterday, please check it and give me feedback soon.";
                self.input.buffer().set_text(original);
                *self.original.borrow_mut() = original.to_string();
                let raw = r#"{"text":"Hi John, I sent the report yesterday. Could you take a look and share your feedback when you have a moment?","mistakes":[{"original":"has send","fix":"sent","category":"tense","context":"Hi John, I has send the report yesterday, please check it and give me feedback soon.","explanation":"Hành động đã xong ở thời điểm xác định (yesterday) → dùng quá khứ đơn."},{"original":"give me feedback soon","fix":"share your feedback when you have a moment","category":"tone","context":"please check it and give me feedback soon.","explanation":"Với khách hàng, câu mệnh lệnh nghe hơi gắt; dùng lời đề nghị lịch sự hơn."}],"tips":[{"phrase":"take a look","meaning":"xem qua","example":"Could you take a look at the draft?"}]}"#;
                let (feedback, structured) = actions::parse_feedback(raw);
                let action = self.actions[1].clone();
                self.show_result(&action, &Outcome { feedback, structured, model: "gpt-demo".into() });
            }
            Ok("busy") => {
                self.busy_label.set_label("Client email: writing… 120 characters");
                self.set_busy(true);
            }
            _ => return false,
        }
        true
    }

    fn focus_actions(&self) {
        if let Some(first) = self.chips.child_at_index(0) {
            first.grab_focus();
        }
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn show_error(&self, text: &str) {
        self.error_label.set_label(text);
        self.error_label.set_visible(true);
    }

    fn show_signin(&self, note: Option<&str>) {
        self.pages.set_visible_child_name("signin");
        self.signin_button.set_sensitive(true);
        self.signin_button.grab_focus();
        self.signin_label.set_label(note.unwrap_or(""));
    }

    fn set_busy(&self, busy: bool) {
        self.chips.set_sensitive(!busy);
        self.busy.set_visible(busy);
    }

    fn cancel(&self) {
        self.request.set(0);
        self.set_busy(false);
        self.toast("Cancelled");
        self.focus_actions();
    }

    fn run_action(self: &Rc<Self>, action: Action) {
        if self.request.get() != 0 {
            return;
        }
        let text = text_of(&self.input);
        if text.trim().is_empty() {
            self.toast("Select some text first, or type it in the box");
            self.input.grab_focus();
            return;
        }
        let id = self.next_request.get() + 1;
        self.next_request.set(id);
        self.request.set(id);
        *self.last_action.borrow_mut() = Some(action.clone());
        self.error_label.set_visible(false);
        self.set_busy(true);
        self.busy_label.set_label(&format!("{}: asking ChatGPT…", action.label));

        let (tx, rx) = async_channel::unbounded::<Msg>();
        let cfg = self.cfg.clone();
        let worker_action = action.clone();
        let worker_text = text.clone();
        gio::spawn_blocking(move || {
            let progress_tx = tx.clone();
            let mut last = 0;
            let result = engine::run(&cfg, &worker_action, &worker_text, &mut |n| {
                if n >= last + 40 {
                    last = n;
                    let _ = progress_tx.send_blocking(Msg::Progress(n));
                }
            });
            let _ = tx.send_blocking(Msg::Done(result));
        });

        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                if this.request.get() != id {
                    return; // cancelled with Esc: drop the answer, don't log it
                }
                match msg {
                    Msg::Progress(n) => {
                        this.busy_label.set_label(&format!("{}: writing… {n} characters", action.label))
                    }
                    Msg::Done(result) => {
                        this.request.set(0);
                        this.set_busy(false);
                        match result {
                            Ok(outcome) => {
                                engine::log(&this.cfg, &action, &text, &outcome);
                                *this.original.borrow_mut() = text.clone();
                                this.show_result(&action, &outcome);
                            }
                            Err(e) if e.downcast_ref::<auth::NotSignedIn>().is_some() => {
                                this.show_signin(Some(&e.to_string()))
                            }
                            Err(e) => {
                                this.show_error(&format!("{e:#}"));
                                this.focus_actions();
                            }
                        }
                        return;
                    }
                }
            }
        });
    }

    fn show_result(&self, action: &Action, outcome: &Outcome) {
        let fb = &outcome.feedback;
        self.result_view.buffer().set_text(&fb.text);
        self.render_diff(&self.original.borrow(), &fb.text);

        self.learn_list.remove_all();
        if !outcome.structured {
            self.learn_list.append(
                &adw::ActionRow::builder()
                    .title("No feedback details this time")
                    .subtitle("The model answered with plain text, so nothing was added to your learning log.")
                    .build(),
            );
        } else if fb.mistakes.is_empty() {
            self.learn_list.append(&adw::ActionRow::builder().title("No mistakes found — nice work!").build());
        }
        for m in &fb.mistakes {
            let row = adw::ActionRow::builder()
                .title(correction_markup(&m.original, &m.fix))
                .subtitle(format!("<b>{}</b> · {}", esc(actions::category_label(&m.category)), esc(&m.explanation)))
                .title_selectable(true)
                .subtitle_selectable(true)
                .build();
            self.learn_list.append(&row);
        }
        for t in &fb.tips {
            let mut subtitle = esc(&t.meaning);
            if !t.example.is_empty() {
                subtitle.push_str(&format!("\n<i>{}</i>", esc(&t.example)));
            }
            let row = adw::ActionRow::builder()
                .title(format!("💡 <b>{}</b>", esc(&t.phrase)))
                .subtitle(subtitle)
                .title_selectable(true)
                .subtitle_selectable(true)
                .build();
            self.learn_list.append(&row);
        }

        let count = fb.mistakes.len();
        self.title.set_subtitle(&format!(
            "{} · {} mistake{} · {}",
            action.label,
            count,
            if count == 1 { "" } else { "s" },
            outcome.model
        ));
        self.result_stack.set_visible_child_name("result");
        self.result_section.set_visible(true);
        self.copy_action.set_enabled(true);
        self.replace_action.set_enabled(true);
        self.has_result.set(true);
        self.primary_hint.set_label("replace");
        // Ready to tweak the wording; Ctrl+Enter sends it.
        self.result_view.grab_focus();
        let buffer = self.result_view.buffer();
        buffer.place_cursor(&buffer.end_iter());
    }

    fn render_diff(&self, old: &str, new: &str) {
        let buffer = self.diff_view.buffer();
        buffer.set_text("");
        let table = buffer.tag_table();
        let (Some(del), Some(ins)) = (table.lookup("del"), table.lookup("ins")) else { return };
        let diff = similar::TextDiff::from_words(old, new);
        let mut iter = buffer.end_iter();
        let mut previous = similar::ChangeTag::Equal;
        for change in diff.iter_all_changes() {
            let value = change.value();
            // Keep a removed word and its replacement apart: "has send sent", not "has sendsent".
            if previous == similar::ChangeTag::Delete && change.tag() == similar::ChangeTag::Insert {
                buffer.insert(&mut iter, " ");
            }
            previous = change.tag();
            match change.tag() {
                similar::ChangeTag::Equal => buffer.insert(&mut iter, value),
                similar::ChangeTag::Delete => buffer.insert_with_tags(&mut iter, value, &[&del]),
                similar::ChangeTag::Insert => buffer.insert_with_tags(&mut iter, value, &[&ins]),
            }
        }
    }

    fn replace(self: &Rc<Self>) {
        if !self.has_result.get() {
            return;
        }
        let text = text_of(&self.result_view);
        self.win.clipboard().set_text(&text);
        if self.cfg.paste_command.is_empty() {
            self.win.close();
            return;
        }
        // Hide so focus returns to the previous app, then send the paste keystroke there.
        self.win.set_visible(false);
        let this = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(350), move || match selection::paste(&this.cfg) {
            Ok(()) => this.win.close(),
            Err(e) => {
                this.win.set_visible(true);
                this.toast(&format!("Copied — press Ctrl+V to paste ({e})"));
            }
        });
    }

    fn sign_in(self: &Rc<Self>) {
        self.signin_button.set_sensitive(false);
        self.signin_label.set_label("Finish signing in in your browser (up to 3 minutes)…");
        self.cancel_signin.store(false, Ordering::Relaxed);
        let cancel = self.cancel_signin.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || auth::sign_in(&cancel, false).map(|c| c.email)).await;
            match result {
                Ok(Ok(email)) => {
                    this.pages.set_visible_child_name("editor");
                    this.win.present();
                    this.focus_actions();
                    this.toast(&if email.is_empty() { "Connected to ChatGPT".to_string() } else { format!("Connected as {email}") });
                    this.welcome_once();
                }
                Ok(Err(e)) => this.show_signin(Some(&format!("{e:#}"))),
                Err(_) => this.show_signin(Some("Sign-in stopped unexpectedly.")),
            }
        });
    }

    /// One-time notice that requests count toward the ChatGPT plan (OpenAI UI guidelines).
    fn welcome_once(&self) {
        let mut state = State::load();
        if state.welcome_shown {
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("You're using your ChatGPT plan"),
            Some("Corrections and drills here count toward your ChatGPT plan's usage limits. You can set a limit for this app under Manage usage."),
        );
        dialog.add_response("usage", "Manage usage");
        dialog.add_response("ok", "Got it");
        dialog.set_default_response(Some("ok"));
        dialog.connect_response(Some("usage"), |_, _| super::open_uri(http::MANAGE_USAGE_URL));
        dialog.present(Some(&self.win));
        state.welcome_shown = true;
        let _ = state.save();
    }
}
