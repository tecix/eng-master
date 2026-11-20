//! User settings (`config.toml`, hand-edited) and app state (`state.json`, written by the app).

use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Model slug. Empty = first model your ChatGPT plan lists.
    pub model: String,
    /// "auto", "low", "medium" or "high".
    pub reasoning_effort: String,
    /// Your English level, given to the model so explanations fit you.
    pub level: String,
    /// Language used for explanations of mistakes and phrase meanings.
    pub explain_language: String,
    /// Who you are and who you write to. Sent with every request.
    pub personal_context: String,
    /// Command run after "Replace" to paste into the previous app. Empty = copy only.
    pub paste_command: Vec<String>,
    /// Also store the full original and result text in the learning log.
    pub log_full_text: bool,
    /// Cards per daily drill.
    pub drill_size: usize,
    /// Notify when drill cards are due.
    pub reminder: bool,
    /// Time of the daily reminder, "HH:MM".
    pub reminder_time: String,
    /// Extra actions, shown after the built-in ones.
    pub actions: Vec<CustomAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomAction {
    pub id: String,
    pub label: String,
    pub task: String,
    /// Symbolic icon name, e.g. "document-edit-symbolic".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: String::new(),
            reasoning_effort: "low".into(),
            level: "B1-B2 (intermediate)".into(),
            explain_language: "Vietnamese".into(),
            personal_context: String::new(),
            // KEY_LEFTCTRL = 29, KEY_V = 47
            paste_command: ["ydotool", "key", "29:1", "47:1", "47:0", "29:0"].map(String::from).to_vec(),
            log_full_text: false,
            drill_size: 8,
            reminder: true,
            reminder_time: "09:30".into(),
            actions: Vec::new(),
        }
    }
}

const TEMPLATE: &str = r#"# en-Master settings. Changes apply to the next window you open.

# Model slug from `enmaster models`. Empty = first model your plan lists.
model = ""

# Thinking effort: "auto", "low", "medium" or "high". "low" is fastest.
reasoning_effort = "low"

# Your level and the language used to explain mistakes.
level = "B1-B2 (intermediate)"
explain_language = "Vietnamese"

# Tell the coach who you are and who you write to. Sent with every request.
# Example: "I'm a software project manager in Vietnam. I email clients in
# Australia and the US and chat with my team on Slack."
personal_context = ""

# Run after "Replace" to paste into the app you came from (ydotool needs ydotoold).
# Set to [] to only copy the result.
paste_command = ["ydotool", "key", "29:1", "47:1", "47:0", "29:0"]

# Keep the full original/result text in the learning log (mistakes are always kept).
log_full_text = false

# Cards per daily drill.
drill_size = 8

# Daily notification when drill cards are due.
reminder = true
reminder_time = "09:30"

# Your own actions. They return the same feedback (text, mistakes, phrases).
# [[actions]]
# id = "apology"
# label = "Apology"
# task = "Rewrite as a sincere, concise apology to a client. Take responsibility, give the fix and the next step."
"#;

pub fn config_path() -> PathBuf {
    paths::config_dir().join("config.toml")
}

/// Load `config.toml`, creating it from the commented template on first run.
pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        paths::ensure_dir(&paths::config_dir())?;
        std::fs::write(&path, TEMPLATE)?;
    }
    let text = std::fs::read_to_string(&path)?;
    toml::from_str(&text).with_context(|| format!("{} is not valid TOML", path.display()))
}

/// Save settings changed in Preferences. Comments in the file are replaced by a short header.
pub fn save(cfg: &Config) -> Result<()> {
    let body = toml::to_string_pretty(cfg)?;
    let text = format!(
        "# en-Master settings. Most of these are also in Preferences (Ctrl+,).\n\
         # Add your own actions as [[actions]] tables with id, label and task.\n\n{body}"
    );
    paths::ensure_dir(&paths::config_dir())?;
    std::fs::write(config_path(), text)?;
    Ok(())
}

/// "HH:MM" as hours and minutes.
pub fn parse_time(text: &str) -> Option<(u32, u32)> {
    let (h, m) = text.split_once(':')?;
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60 && text.len() == 5).then_some((h, m))
}

/// Things the app learns at run time; not meant for hand editing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Model picked automatically when `config.model` is empty.
    pub model: String,
    pub welcome_shown: bool,
    /// Whether the plan endpoint accepted `text.format: json_schema`.
    pub schema_supported: Option<bool>,
    /// Whether the plan endpoint accepted `reasoning.effort` for `model`.
    pub effort_supported: Option<bool>,
    /// Day the drill reminder was last shown, so it fires once a day.
    pub reminded_on: Option<chrono::NaiveDate>,
}

fn state_path() -> PathBuf {
    paths::data_dir().join("state.json")
}

impl State {
    pub fn load() -> State {
        std::fs::read(state_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        paths::write_private(&state_path(), &serde_json::to_vec_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_settings_load_back() {
        let cfg = Config {
            personal_context: "PM \"quotes\"".into(),
            actions: vec![CustomAction { id: "a".into(), label: "A".into(), task: "t".into(), icon: None }],
            ..Config::default()
        };
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }

    #[test]
    fn times_are_validated() {
        assert_eq!(parse_time("09:30"), Some((9, 30)));
        assert_eq!(parse_time("9:30"), None);
        assert_eq!(parse_time("24:00"), None);
    }

    #[test]
    fn template_matches_defaults() {
        let parsed: Config = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(parsed, Config::default());
    }
}
