//! The coach window: your text, the actions, the result, the changes and what to learn.
//! Keyboard first: Alt+1…9 run actions, Ctrl+Enter runs Coach or replaces, Esc cancels or closes.

use super::{correction_markup, esc, hint, hints, row_icon, win_action, with_keys};
use crate::actions::{self, Action};
use crate::config::{self, Config, State};
use crate::engine::{self, Outcome};
use crate::i18n::{ntrf, tr, trf};
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
    error_box: gtk::Box,
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
    /// Replace pastes into the previous app; otherwise it copies and closes.
    can_paste: bool,
    original: RefCell<String>,
    last_action: RefCell<Option<Action>>,
    /// Action requested on the command line, run once there is text and a connection.
    pending_action: RefCell<Option<Action>>,
    signed_in: Cell<bool>,
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
    // Outside the sandbox, read the selection before our window takes focus; inside it, GTK
    // reads it once the window is focused.
    let (text, source) = if paths::sandboxed() { (String::new(), Source::Empty) } else { selection::capture() };
    let coach = Coach::new(app, cfg);
    coach.input.buffer().set_text(&text);
    coach.source_chip.set_label(&source_label(source));
    super::present(&coach.win);
    super::maybe_snapshot(&coach.win);
    #[cfg(debug_assertions)]
    if coach.demo() {
        return;
    }
    if source == Source::Empty {
        coach.input.grab_focus();
        coach.read_selection_when_focused();
    } else {
        coach.focus_actions();
    }
    if let Some(error) = cfg_error {
        coach.show_error(&trf("The settings file has an error, so defaults are used: {}", &[&error]));
    }
    if let Some(id) = action_id {
        match coach.actions.iter().find(|a| a.id == id).cloned() {
            Some(action) => *coach.pending_action.borrow_mut() = Some(action),
            None => coach.show_error(&trf("Unknown action “{}”", &[&id])),
        }
    }

    let coach2 = coach.clone();
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(auth::status).await {
            Ok(Ok(auth::Status::SignedIn { plan_access: true, .. })) => {
                coach2.signed_in.set(true);
                coach2.run_pending();
            }
            Ok(Ok(_)) => coach2.show_signin(None),
            Ok(Err(e)) => coach2.show_error(&format!("{e:#}")),
            Err(_) => coach2.show_error(&tr("Could not check the ChatGPT connection")),
        }
    });
}

fn source_label(source: Source) -> String {
    match source {
        Source::Selection => tr("From your selection"),
        Source::Clipboard => tr("From the clipboard"),
        Source::Empty => tr("Type or paste your text"),
    }
}

fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
}

fn text_view(editable: bool, name: &str) -> gtk::TextView {
    let view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .editable(editable)
        .cursor_visible(editable)
        .accepts_tab(false)
        .top_margin(10)
        .bottom_margin(10)
        .left_margin(12)
        .right_margin(12)
        .build();
    view.update_property(&[gtk::accessible::Property::Label(name)]);
    view
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

fn icon_button(icon: &str, action: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder().icon_name(icon).action_name(action).tooltip_text(tooltip).build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

impl Coach {
    fn new(app: &adw::Application, cfg: Config) -> Rc<Coach> {
        let actions = actions::all(&cfg);
        let can_paste = !paths::sandboxed() && !cfg.paste_command.is_empty();
        let mut narrow_hidden: Vec<gtk::Widget> = Vec::new();

        // Header: primary menu per the HIG (no Quit; Ctrl+Q still works).
        let title = adw::WindowTitle::new(paths::APP_NAME, "");
        let header = adw::HeaderBar::builder().title_widget(&title).build();
        let menu = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some(&tr("_Progress")), Some("app.progress"));
        section.append(Some(&tr("_Daily Drill")), Some("app.drill"));
        menu.append_section(None, &section);
        let section = gio::Menu::new();
        section.append(Some(&tr("_Preferences")), Some("app.preferences"));
        section.append(Some(&tr("_Keyboard Shortcuts")), Some("app.shortcuts"));
        section.append(Some(&tr("_About en-Master")), Some("app.about"));
        menu.append_section(None, &section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .primary(true)
            .tooltip_text(tr("Main Menu"))
            .build();
        menu_button.update_property(&[gtk::accessible::Property::Label(&tr("Main Menu"))]);
        header.pack_end(&menu_button);
        header.pack_end(&icon_button(
            "preferences-desktop-keyboard-shortcuts-symbolic",
            "app.shortcuts",
            &tr("Keyboard Shortcuts"),
        ));

        // Your text
        let source_chip = gtk::Label::builder().css_classes(["source-chip", "dim-label"]).build();
        let input_head = gtk::Box::builder().spacing(8).build();
        input_head.append(&gtk::Label::builder().label(tr("Your Text")).xalign(0.0).css_classes(["heading"]).build());
        input_head.append(&source_chip);
        input_head.append(&gtk::Box::builder().hexpand(true).build());
        let edit_hint = hint("<Control>e", &tr("edit"));
        input_head.append(&edit_hint);
        narrow_hidden.push(edit_hint.upcast());
        let input = text_view(true, &tr("Your text"));

        // Action chips: arrows + Enter, Alt+N, or click.
        let chips = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .activate_on_single_click(true)
            .column_spacing(8)
            .row_spacing(8)
            .max_children_per_line(8)
            .css_classes(["actions"])
            .build();
        chips.update_property(&[gtk::accessible::Property::Label(&tr("Actions"))]);
        for (i, action) in actions.iter().enumerate() {
            let row = gtk::Box::builder().spacing(8).build();
            row.append(&gtk::Image::from_icon_name(&action.icon));
            row.append(&gtk::Label::new(Some(&action.label)));
            if i < 9 {
                let key = super::keycap(&format!("<Alt>{}", i + 1));
                row.append(&key);
                narrow_hidden.push(key.upcast());
            }
            let chip = gtk::FlowBoxChild::builder().child(&row).css_classes(["action-chip"]).build();
            if i == 0 {
                chip.add_css_class("primary");
            }
            chips.append(&chip);
        }
        let (hint_row, hint_labels) = hints(&[
            ("<Control>Return", &tr("run Coach")),
            ("<Control>k", &tr("pick with arrow keys")),
            ("<Control>r", &tr("run again")),
            ("F1", &tr("all shortcuts")),
        ]);
        narrow_hidden.push(hint_row.clone().upcast());
        let primary_hint = hint_labels[0].clone();

        let busy_label = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).css_classes(["dim-label"]).build();
        let busy = gtk::Box::builder().spacing(8).visible(false).build();
        busy.append(&adw::Spinner::new());
        busy.append(&busy_label);
        busy.append(&hint("Escape", &tr("cancel")));
        let error_label = gtk::Label::builder().xalign(0.0).hexpand(true).wrap(true).selectable(true).build();
        let error_box = gtk::Box::builder().spacing(8).css_classes(["error"]).visible(false).build();
        error_box.append(&gtk::Image::builder().icon_name("dialog-warning-symbolic").valign(gtk::Align::Start).build());
        error_box.append(&error_label);

        // Result / Changes
        let result_view = text_view(true, &tr("Result"));
        let diff_view = text_view(false, &tr("Changes"));
        let result_stack = adw::ViewStack::new();
        result_stack.add_titled_with_icon(
            &framed(&result_view, 90),
            Some("result"),
            &tr("Result"),
            "document-edit-symbolic",
        );
        result_stack.add_titled_with_icon(
            &framed(&diff_view, 90),
            Some("changes"),
            &tr("Changes"),
            "view-dual-symbolic",
        );
        let switcher_row = gtk::Box::builder().spacing(8).build();
        switcher_row.append(
            &adw::InlineViewSwitcher::builder().stack(&result_stack).hexpand(true).halign(gtk::Align::Start).build(),
        );
        let switch_hint = hint("<Control>d", &tr("switch"));
        switcher_row.append(&switch_hint);
        narrow_hidden.push(switch_hint.upcast());
        let learn_list =
            gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        let result_section =
            gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).visible(false).build();
        result_section.append(&switcher_row);
        result_section.append(&result_stack);
        result_section.append(&heading(&tr("What to Learn")));
        result_section.append(&learn_list);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(12)
            .margin_end(12)
            .build();
        column.append(&input_head);
        column.append(&framed(&input, 90));
        column.append(&chips);
        column.append(&hint_row);
        column.append(&busy);
        column.append(&error_box);
        column.append(&result_section);
        let clamp = adw::Clamp::builder().maximum_size(860).tightening_threshold(700).child(&column).build();
        let editor = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();

        // Sign-in page
        let signin_button = gtk::Button::builder()
            .label(tr("Continue with ChatGPT"))
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let signin_label =
            gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).css_classes(["dim-label"]).build();
        let signin_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        signin_box.append(&signin_button);
        signin_box.append(&signin_label);
        let signin = adw::StatusPage::builder()
            .icon_name(paths::APP_ID)
            .title(tr("Connect Your ChatGPT Plan"))
            .description(tr(
                "en-Master uses your ChatGPT plan, so no API key is needed. Signing in opens your web browser.",
            ))
            .child(&signin_box)
            .build();

        let pages = gtk::Stack::new();
        pages.add_named(&editor, Some("editor"));
        pages.add_named(&signin, Some("signin"));
        pages.set_visible_child_name("editor");

        // Bottom bar: plan notice (OpenAI UI guidelines) and result buttons with their keys.
        let plan = gtk::Label::builder()
            .use_markup(true)
            .label(trf("Using ChatGPT plan · <a href=\"{}\">Manage usage</a>", &[http::MANAGE_USAGE_URL]))
            .margin_start(6)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();
        let (copy_content, copy_key) = with_keys(&tr("_Copy"), "<Control><Shift>c");
        let copy_button = gtk::Button::builder().child(&copy_content).action_name("win.copy").build();
        let replace_text = if can_paste { tr("_Replace") } else { tr("Copy and _Close") };
        let (replace_content, replace_key) = with_keys(&replace_text, "<Control>Return");
        let replace_button = gtk::Button::builder()
            .child(&replace_content)
            .action_name("win.replace")
            .tooltip_text(if can_paste {
                tr("Paste into the app you came from")
            } else {
                tr("Copy the result and close")
            })
            .css_classes(["suggested-action"])
            .build();
        narrow_hidden.push(copy_key.upcast());
        narrow_hidden.push(replace_key.upcast());
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
            .width_request(360)
            .height_request(294)
            .content(&toasts)
            .build();
        win.set_widget_name(WINDOW_NAME);

        // Narrow windows (phones, tiled halves): drop keyboard hints and shorten the plan notice.
        let narrow =
            adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 560sp").expect("valid condition"));
        for widget in &narrow_hidden {
            narrow.add_setter(widget, "visible", Some(&false.to_value()));
        }
        let short_plan = format!("<a href=\"{}\">{}</a>", http::MANAGE_USAGE_URL, esc(&tr("Manage usage")));
        narrow.add_setter(&plan, "label", Some(&short_plan.to_value()));
        win.add_breakpoint(narrow);

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
            error_box,
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
            can_paste,
            original: RefCell::new(String::new()),
            last_action: RefCell::new(None),
            pending_action: RefCell::new(None),
            signed_in: Cell::new(false),
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
            this.toast(&tr("Copied"));
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
                None => this.toast(&tr("No action to repeat yet")),
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

        let cancel = self.cancel_signin.clone();
        self.win.connect_close_request(move |_| {
            cancel.store(true, Ordering::Relaxed);
            glib::Propagation::Proceed
        });
    }

    /// Debug builds only: `ENMASTER_DEMO=result|screenshot|signin|busy` shows a page without network.
    #[cfg(debug_assertions)]
    fn demo(&self) -> bool {
        let english = std::env::var("ENMASTER_DEMO").as_deref() == Ok("screenshot");
        match std::env::var("ENMASTER_DEMO").as_deref() {
            Ok("signin") => self.show_signin(None),
            Ok("result" | "screenshot") => {
                let original = "Hi John, I has send the report yesterday, please check it and give me feedback soon.";
                self.input.buffer().set_text(original);
                self.source_chip.set_label(&source_label(Source::Selection));
                *self.original.borrow_mut() = original.to_string();
                let (tense, tone, tip) = if english {
                    (
                        "The action finished at a stated time (yesterday), so use the simple past.",
                        "An order sounds abrupt to a client; a polite request works better.",
                        "have a quick look",
                    )
                } else {
                    (
                        "Hành động đã xong ở thời điểm xác định (yesterday) → dùng quá khứ đơn.",
                        "Với khách hàng, câu mệnh lệnh nghe hơi gắt; dùng lời đề nghị lịch sự hơn.",
                        "xem qua",
                    )
                };
                let raw = serde_json::json!({
                    "text": "Hi John, I sent the report yesterday. Could you take a look and share your feedback when you have a moment?",
                    "mistakes": [
                        {"original": "has send", "fix": "sent", "category": "tense", "context": original, "explanation": tense},
                        {"original": "give me feedback soon", "fix": "share your feedback when you have a moment", "category": "tone", "context": original, "explanation": tone}
                    ],
                    "tips": [{"phrase": "take a look", "meaning": tip, "example": "Could you take a look at the draft?"}]
                })
                .to_string();
                let (feedback, structured) = actions::parse_feedback(&raw);
                let action = self.actions[1].clone();
                self.show_result(&action, &Outcome { feedback, structured, model: "gpt-6.2-sol".into() });
            }
            Ok("busy") => {
                self.busy_label.set_label(&trf("{}: writing… {} characters", &[&self.actions[1].label, "120"]));
                self.set_busy(true);
            }
            _ => return false,
        }
        true
    }

    /// Inside the sandbox (or when the host tools found nothing), read the selection through GTK
    /// once the window has focus, then fall back to the clipboard.
    fn read_selection_when_focused(self: &Rc<Self>) {
        let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::new(RefCell::new(None));
        let handler2 = handler.clone();
        let this = self.clone();
        let id = self.win.connect_is_active_notify(move |win| {
            if !win.is_active() {
                return;
            }
            if let Some(id) = handler2.borrow_mut().take() {
                win.disconnect(id);
            }
            let this = this.clone();
            glib::spawn_future_local(async move { this.fill_from_clipboards().await });
        });
        *handler.borrow_mut() = Some(id);
    }

    async fn fill_from_clipboards(self: Rc<Self>) {
        let display = WidgetExt::display(&self.win);
        let read = |clipboard: gtk::gdk::Clipboard| async move {
            clipboard.read_text_future().await.ok().flatten().map(|t| t.to_string()).filter(|t| !t.trim().is_empty())
        };
        let found = match read(display.primary_clipboard()).await {
            Some(text) => Some((text, Source::Selection)),
            None => read(display.clipboard()).await.map(|text| (text, Source::Clipboard)),
        };
        let Some((text, source)) = found else { return };
        if !text_of(&self.input).trim().is_empty() {
            return; // the user started typing
        }
        self.input.buffer().set_text(&text);
        self.source_chip.set_label(&source_label(source));
        self.focus_actions();
        self.run_pending();
    }

    /// Run the action from the command line once there is text and a ChatGPT connection.
    fn run_pending(self: &Rc<Self>) {
        if !self.signed_in.get() || text_of(&self.input).trim().is_empty() {
            return;
        }
        let pending = self.pending_action.borrow_mut().take();
        if let Some(action) = pending {
            self.run_action(action);
        }
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
        self.error_box.set_visible(true);
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
        self.toast(&tr("Cancelled"));
        self.focus_actions();
    }

    fn run_action(self: &Rc<Self>, action: Action) {
        if self.request.get() != 0 {
            return;
        }
        let text = text_of(&self.input);
        if text.trim().is_empty() {
            self.toast(&tr("Select some text first, or type it in the box"));
            self.input.grab_focus();
            return;
        }
        let id = self.next_request.get() + 1;
        self.next_request.set(id);
        self.request.set(id);
        *self.last_action.borrow_mut() = Some(action.clone());
        self.error_box.set_visible(false);
        self.set_busy(true);
        self.busy_label.set_label(&trf("{}: asking ChatGPT…", &[&action.label]));

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
                        this.busy_label.set_label(&trf("{}: writing… {} characters", &[&action.label, &n.to_string()]))
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

    fn learn_row(&self, icon: &str, title: &str, subtitle: &str) {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .title_selectable(true)
            .subtitle_selectable(true)
            .build();
        row.add_prefix(&row_icon(icon));
        self.learn_list.append(&row);
    }

    fn show_result(&self, action: &Action, outcome: &Outcome) {
        let fb = &outcome.feedback;
        self.result_view.buffer().set_text(&fb.text);
        self.render_diff(&self.original.borrow(), &fb.text);

        self.learn_list.remove_all();
        if !outcome.structured {
            self.learn_row(
                "dialog-information-symbolic",
                &esc(&tr("No feedback details this time")),
                &esc(&tr("The model answered with plain text, so nothing was added to your learning log.")),
            );
        } else if fb.mistakes.is_empty() {
            self.learn_row("object-select-symbolic", &esc(&tr("No mistakes found. Nice work!")), "");
        }
        for m in &fb.mistakes {
            self.learn_row(
                "tools-check-spelling-symbolic",
                &correction_markup(&m.original, &m.fix),
                &format!("<b>{}</b> · {}", esc(&actions::category_label(&m.category)), esc(&m.explanation)),
            );
        }
        for t in &fb.tips {
            let mut subtitle = esc(&t.meaning);
            if !t.example.is_empty() {
                subtitle.push_str(&format!("\n<i>{}</i>", esc(&t.example)));
            }
            self.learn_row("enmaster-lightbulb-symbolic", &format!("<b>{}</b>", esc(&t.phrase)), &subtitle);
        }

        let count = fb.mistakes.len() as u32;
        let mistakes = ntrf("{} mistake", "{} mistakes", count, &[&count.to_string()]);
        self.title.set_subtitle(&format!("{} · {} · {}", action.label, mistakes, outcome.model));
        self.result_stack.set_visible_child_name("result");
        self.result_section.set_visible(true);
        self.copy_action.set_enabled(true);
        self.replace_action.set_enabled(true);
        self.has_result.set(true);
        self.primary_hint.set_label(&if self.can_paste { tr("replace") } else { tr("copy and close") });
        // Ready to tweak the wording; Ctrl+Enter sends it.
        self.result_view.grab_focus();
        let buffer = self.result_view.buffer();
        buffer.place_cursor(&buffer.end_iter());
    }

    fn render_diff(&self, old: &str, new: &str) {
        let buffer = self.diff_view.buffer();
        buffer.set_text("");
        let (red, green) = super::palette();
        let table = buffer.tag_table();
        for name in ["del", "ins"] {
            if let Some(tag) = table.lookup(name) {
                table.remove(&tag);
            }
        }
        let del = buffer.create_tag(Some("del"), &[("foreground", &red), ("strikethrough", &true)]);
        let ins = buffer.create_tag(Some("ins"), &[("foreground", &green), ("weight", &700i32)]);
        let (Some(del), Some(ins)) = (del, ins) else { return };
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
        if !self.can_paste {
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
                this.toast(&trf("Copied. Press Ctrl+V to paste ({})", &[&e.to_string()]));
            }
        });
    }

    fn sign_in(self: &Rc<Self>) {
        self.signin_button.set_sensitive(false);
        self.signin_label.set_label(&tr("Finish signing in in your web browser…"));
        self.cancel_signin.store(false, Ordering::Relaxed);
        let cancel = self.cancel_signin.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || auth::sign_in(&cancel, false).map(|c| c.email)).await;
            match result {
                Ok(Ok(email)) => {
                    this.signed_in.set(true);
                    this.pages.set_visible_child_name("editor");
                    this.win.present();
                    this.focus_actions();
                    this.toast(&if email.is_empty() {
                        tr("Connected to ChatGPT")
                    } else {
                        trf("Connected as {}", &[&email])
                    });
                    welcome_once(this.win.upcast_ref());
                    this.run_pending();
                }
                Ok(Err(e)) => this.show_signin(Some(&format!("{e:#}"))),
                Err(_) => this.show_signin(Some(&tr("Signing in stopped unexpectedly"))),
            }
        });
    }
}

/// One-time notice that requests count toward the ChatGPT plan (OpenAI UI guidelines).
pub fn welcome_once(parent: &gtk::Widget) {
    let mut state = State::load();
    if state.welcome_shown {
        return;
    }
    let dialog = adw::AlertDialog::new(
        Some(&tr("You’re Using Your ChatGPT Plan")),
        Some(&tr(
            "Corrections and drills count toward your ChatGPT plan’s usage limits. You can set a limit for this app under Manage Usage.",
        )),
    );
    dialog.add_response("usage", &tr("_Manage Usage"));
    dialog.add_response("ok", &tr("_Got It"));
    dialog.set_default_response(Some("ok"));
    dialog.connect_response(Some("usage"), |_, _| super::open_uri(http::MANAGE_USAGE_URL));
    dialog.present(Some(parent));
    state.welcome_shown = true;
    let _ = state.save();
}
