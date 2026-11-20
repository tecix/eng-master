mod actions;
mod api;
mod auth;
mod config;
mod engine;
mod http;
mod i18n;
mod install;
mod keyring;
mod learn;
mod paths;
mod selection;
mod srs;
mod ui;

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

const HELP: &str = "en-Master — practise English writing on any selected text (GNOME).

Window (usually from a GNOME shortcut):
  enmaster                    open on the selected text          Super+Alt+E
  enmaster --action ID        run an action right away           Super+Alt+C (coach)
                              (coach, email, chat, social, natural, translate, or your own)
  enmaster drill              today's drill of your past mistakes Super+Alt+D
  enmaster progress           progress report
  enmaster --background       start without a window (reminders, shortcuts)
  enmaster quit               stop the background instance
  In the window, F1 lists every keyboard shortcut.

Account (ChatGPT plan, no API key):
  enmaster login [--register] Continue with ChatGPT in the browser
                              (--register: register this app with ChatGPT again)
  enmaster logout             sign out and revoke the token
  enmaster status             connection and model
  enmaster models             models your plan can use

Terminal:
  enmaster run ID             read text on stdin, print the result
  enmaster report [DAYS]      progress report (default 7 days)
  enmaster anki [FILE]        export mistakes and phrases for Anki
  enmaster install            install to ~/.local (without Flatpak): desktop entry,
                              icons, translations, GNOME shortcuts, autostart
  enmaster uninstall          remove what install added (keeps your data)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    paths::migrate_legacy();
    i18n::init();
    let command = args.get(1).map(String::as_str).unwrap_or("");
    let result = match command {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(())
        }
        "login" => login(args.iter().any(|a| a == "--register")),
        "logout" => logout(),
        "status" => status(),
        "models" => list_models(),
        "run" => run_stdin(args.get(2).map(String::as_str).unwrap_or("coach")),
        "report" => report(args.get(2).map(String::as_str)),
        "anki" => anki(args.get(2).map(String::as_str)),
        "install" => install::install(),
        "uninstall" => install::uninstall(),
        _ => return ui::run(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("enmaster: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn login(register_again: bool) -> Result<()> {
    let creds = auth::sign_in(&AtomicBool::new(false), register_again)?;
    println!("Connected to ChatGPT as {}.", if creds.email.is_empty() { "your account" } else { &creds.email });
    println!("You're using your ChatGPT plan. Manage usage: {}", http::MANAGE_USAGE_URL);
    Ok(())
}

fn logout() -> Result<()> {
    if auth::sign_out()? {
        println!("Signed out.");
    } else {
        println!(
            "Signed out locally. The token could not be revoked online; you can disconnect the app in ChatGPT settings."
        );
    }
    Ok(())
}

fn status() -> Result<()> {
    let cfg = config::load()?;
    match auth::status()? {
        auth::Status::SignedOut => println!("Not connected. Run `enmaster login`."),
        auth::Status::SignedIn { email, plan_access } => {
            println!("Connected to ChatGPT{}.", if email.is_empty() { String::new() } else { format!(" as {email}") });
            if !plan_access {
                println!("Plan usage was not granted — run `enmaster login` again and allow it.");
            }
        }
    }
    let state = config::State::load();
    let model = if !cfg.model.is_empty() {
        cfg.model.clone()
    } else if !state.model.is_empty() {
        format!("{} (auto)", state.model)
    } else {
        "auto".into()
    };
    println!("Model: {model} · effort: {}", cfg.reasoning_effort);
    println!("Settings: {}", config::config_path().display());
    println!("Learning log: {} ({} texts)", learn::log_path().display(), learn::load().len());
    Ok(())
}

fn list_models() -> Result<()> {
    let token = auth::access_token()?;
    for model in api::models(auth::RESOURCE, &token)? {
        if model.name == model.id {
            println!("{}", model.id);
        } else {
            println!("{}\t{}", model.id, model.name);
        }
    }
    Ok(())
}

fn run_stdin(action_id: &str) -> Result<()> {
    let cfg = config::load()?;
    let Some(action) = actions::find(&cfg, action_id) else {
        let ids: Vec<String> = actions::all(&cfg).into_iter().map(|a| a.id).collect();
        bail!("unknown action {action_id:?}; use one of: {}", ids.join(", "));
    };
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    let outcome = engine::run_and_log(&cfg, &action, &text, &mut |_| {})?;
    println!("{}", outcome.feedback.text);
    for m in &outcome.feedback.mistakes {
        eprintln!("✗ {} → {}  [{}] {}", m.original, m.fix, actions::category_label(&m.category), m.explanation);
    }
    for t in &outcome.feedback.tips {
        eprintln!("💡 {} — {}", t.phrase, t.meaning);
    }
    Ok(())
}

fn report(days: Option<&str>) -> Result<()> {
    let days: i64 = days.map(str::parse).transpose().context("DAYS must be a number")?.unwrap_or(7);
    print!("{}", learn::render_text(&learn::report(&learn::load(), days, chrono::Local::now())));
    Ok(())
}

fn anki(file: Option<&str>) -> Result<()> {
    let path = file.map(std::path::PathBuf::from).unwrap_or_else(|| {
        dirs::document_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default()).join("enmaster-anki.txt")
    });
    let (text, count) = learn::anki_export(&learn::load());
    std::fs::write(&path, text)?;
    println!("Wrote {count} cards to {}. In Anki: File → Import.", path.display());
    Ok(())
}
