//! Learning log (`log.jsonl`), progress reports and Anki export.

use crate::actions::{self, Feedback, Mistake, Tip};
use crate::config::Config;
use crate::paths;
use anyhow::Result;
use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub ts: DateTime<Local>,
    pub action: String,
    #[serde(default)]
    pub model: String,
    /// Words in the original text.
    #[serde(default)]
    pub words: usize,
    #[serde(default)]
    pub mistakes: Vec<Mistake>,
    #[serde(default)]
    pub tips: Vec<Tip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

pub fn log_path() -> PathBuf {
    paths::data_dir().join("log.jsonl")
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count()
}

pub fn record(cfg: &Config, action: &str, model: &str, original: &str, feedback: &Feedback) -> Result<()> {
    let entry = Entry {
        ts: Local::now(),
        action: action.to_string(),
        model: model.to_string(),
        words: word_count(original),
        mistakes: feedback.mistakes.clone(),
        tips: feedback.tips.clone(),
        original: cfg.log_full_text.then(|| original.to_string()),
        result: cfg.log_full_text.then(|| feedback.text.clone()),
    };
    paths::append_private(&log_path(), &serde_json::to_string(&entry)?)
}

/// All readable entries, oldest first. Damaged lines are skipped.
pub fn load() -> Vec<Entry> {
    std::fs::read_to_string(log_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Lowercase, single-spaced, without surrounding punctuation or curly quotes.
pub fn normalize(text: &str) -> String {
    text.replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201C}', '\u{201D}'], "\"")
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| c.is_ascii_punctuation() && c != '\'')
        .to_string()
}

#[derive(Debug, Clone, PartialEq)]
pub struct CategoryStat {
    pub id: String,
    pub count: usize,
    pub previous: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Repeated {
    pub original: String,
    pub fix: String,
    pub explanation: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub days: i64,
    pub sessions: usize,
    pub words: usize,
    pub mistakes: usize,
    pub previous_sessions: usize,
    pub previous_words: usize,
    pub previous_mistakes: usize,
    pub categories: Vec<CategoryStat>,
    pub repeated: Vec<Repeated>,
    pub tips: Vec<Tip>,
}

impl Report {
    pub fn per_100_words(&self) -> Option<f64> {
        (self.words > 0).then(|| self.mistakes as f64 * 100.0 / self.words as f64)
    }

    pub fn previous_per_100_words(&self) -> Option<f64> {
        (self.previous_words > 0).then(|| self.previous_mistakes as f64 * 100.0 / self.previous_words as f64)
    }
}

pub fn report(entries: &[Entry], days: i64, now: DateTime<Local>) -> Report {
    let start = now - Duration::days(days);
    let previous_start = start - Duration::days(days);
    let current: Vec<&Entry> = entries.iter().filter(|e| e.ts > start && e.ts <= now).collect();
    let previous: Vec<&Entry> = entries.iter().filter(|e| e.ts > previous_start && e.ts <= start).collect();

    let mut by_category: HashMap<&str, (usize, usize)> = HashMap::new();
    for m in current.iter().flat_map(|e| &e.mistakes) {
        by_category.entry(m.category.as_str()).or_default().0 += 1;
    }
    for m in previous.iter().flat_map(|e| &e.mistakes) {
        by_category.entry(m.category.as_str()).or_default().1 += 1;
    }
    let mut categories: Vec<CategoryStat> = by_category
        .into_iter()
        .filter(|(_, (count, _))| *count > 0)
        .map(|(id, (count, previous))| CategoryStat { id: id.to_string(), count, previous })
        .collect();
    categories.sort_by(|a, b| b.count.cmp(&a.count).then(a.id.cmp(&b.id)));

    let mut repeated: HashMap<(String, String), Repeated> = HashMap::new();
    for m in current.iter().flat_map(|e| &e.mistakes) {
        let key = (normalize(&m.original), normalize(&m.fix));
        let item = repeated.entry(key).or_insert_with(|| Repeated {
            original: m.original.clone(),
            fix: m.fix.clone(),
            explanation: m.explanation.clone(),
            count: 0,
        });
        item.count += 1;
    }
    let mut repeated: Vec<Repeated> = repeated.into_values().filter(|r| r.count >= 2).collect();
    repeated.sort_by(|a, b| b.count.cmp(&a.count).then(a.original.cmp(&b.original)));
    repeated.truncate(8);

    let mut seen = std::collections::HashSet::new();
    let tips: Vec<Tip> = current
        .iter()
        .rev()
        .flat_map(|e| &e.tips)
        .filter(|t| seen.insert(normalize(&t.phrase)))
        .take(10)
        .cloned()
        .collect();

    let count = |list: &[&Entry]| list.iter().map(|e| e.mistakes.len()).sum();
    let words = |list: &[&Entry]| list.iter().map(|e| e.words).sum();
    Report {
        days,
        sessions: current.len(),
        words: words(&current),
        mistakes: count(&current),
        previous_sessions: previous.len(),
        previous_words: words(&previous),
        previous_mistakes: count(&previous),
        categories,
        repeated,
        tips,
    }
}

pub fn render_text(r: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "en-Master — last {} days", r.days);
    let _ = writeln!(out, "  Texts checked: {} (previous period: {})", r.sessions, r.previous_sessions);
    let _ = writeln!(out, "  Words written: {}", r.words);
    let rate = |v: Option<f64>| v.map(|v| format!("{v:.1}")).unwrap_or_else(|| "–".into());
    let _ = writeln!(
        out,
        "  Mistakes: {} · per 100 words: {} (previous: {})",
        r.mistakes,
        rate(r.per_100_words()),
        rate(r.previous_per_100_words())
    );
    if !r.categories.is_empty() {
        let _ = writeln!(out, "\nMistake types:");
        for c in &r.categories {
            let _ = writeln!(out, "  {:<26} {:>3}  (previous {})", actions::category_label(&c.id), c.count, c.previous);
        }
    }
    if !r.repeated.is_empty() {
        let _ = writeln!(out, "\nRepeated mistakes:");
        for m in &r.repeated {
            let _ = writeln!(out, "  {}× \"{}\" → \"{}\"  {}", m.count, m.original, m.fix, m.explanation);
        }
    }
    if !r.tips.is_empty() {
        let _ = writeln!(out, "\nPhrases to remember:");
        for t in &r.tips {
            let _ = writeln!(out, "  • {} — {}", t.phrase, t.meaning);
        }
    }
    if r.sessions == 0 {
        let _ = writeln!(out, "\nNo texts checked in this period yet.");
    }
    out
}

fn anki_field(text: &str) -> String {
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    escaped.replace(['\t', '\r'], " ").replace('\n', "<br>")
}

/// Anki "Notes in plain text" export: tab-separated front, back and tags.
pub fn anki_export(entries: &[Entry]) -> (String, usize) {
    let mut out = String::from("#separator:tab\n#html:true\n#tags column:3\n");
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for m in entries.iter().rev().flat_map(|e| &e.mistakes) {
        if !seen.insert(("m", normalize(&m.original), normalize(&m.fix))) {
            continue;
        }
        let context = if m.context.is_empty() { m.original.clone() } else { m.context.clone() };
        let highlighted = anki_field(&context).replacen(
            &anki_field(&m.original),
            &format!("<b><u>{}</u></b>", anki_field(&m.original)),
            1,
        );
        let front = format!("Fix the mistake:<br>{highlighted}");
        let back = format!(
            "<b>{}</b> → <b>{}</b><br>{}",
            anki_field(&m.original),
            anki_field(&m.fix),
            anki_field(&m.explanation)
        );
        let _ = writeln!(out, "{front}\t{back}\tenmaster mistake {}", m.category);
        count += 1;
    }
    for t in entries.iter().rev().flat_map(|e| &e.tips) {
        if !seen.insert(("t", normalize(&t.phrase), String::new())) {
            continue;
        }
        let front = format!("Say in English:<br>{}", anki_field(&t.meaning));
        let back = format!("<b>{}</b><br><i>{}</i>", anki_field(&t.phrase), anki_field(&t.example));
        let _ = writeln!(out, "{front}\t{back}\tenmaster phrase");
        count += 1;
    }
    (out, count)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn mistake(original: &str, fix: &str, category: &str) -> Mistake {
        Mistake {
            original: original.into(),
            fix: fix.into(),
            category: category.into(),
            context: format!("I sent {original} yesterday."),
            explanation: "giải thích".into(),
        }
    }

    pub fn entry(days_ago: i64, mistakes: Vec<Mistake>) -> Entry {
        Entry {
            ts: Local::now() - Duration::days(days_ago) - Duration::minutes(1),
            action: "coach".into(),
            model: "m".into(),
            words: 50,
            mistakes,
            tips: vec![Tip {
                phrase: "touch base".into(),
                meaning: "liên lạc".into(),
                example: "Let's touch base.".into(),
            }],
            original: None,
            result: None,
        }
    }

    #[test]
    fn report_compares_periods_and_finds_repeats() {
        let entries = vec![
            entry(10, vec![mistake("the email", "an email", "article")]),
            entry(
                2,
                vec![mistake("informations", "information", "plural"), mistake("the email", "an email", "article")],
            ),
            entry(1, vec![mistake("The email", "an email", "article")]),
        ];
        let r = report(&entries, 7, Local::now());
        assert_eq!(r.sessions, 2);
        assert_eq!(r.previous_sessions, 1);
        assert_eq!(r.mistakes, 3);
        assert_eq!(r.categories[0], CategoryStat { id: "article".into(), count: 2, previous: 1 });
        assert_eq!(r.repeated.len(), 1);
        assert_eq!(r.repeated[0].count, 2);
        assert_eq!(r.tips.len(), 1);
        assert!((r.per_100_words().unwrap() - 3.0).abs() < 1e-9);
        assert!(render_text(&r).contains("Articles"));
    }

    #[test]
    fn anki_export_dedupes_and_escapes() {
        let entries = vec![
            entry(1, vec![mistake("a <b>", "the\tb", "article")]),
            entry(0, vec![mistake("A <b>", "the\tb", "article")]),
        ];
        let (text, count) = anki_export(&entries);
        assert_eq!(count, 2); // one mistake + one phrase
        assert!(text.contains("&lt;b&gt;"));
        assert_eq!(text.lines().filter(|l| !l.starts_with('#')).count(), 2);
        assert!(text.lines().skip(3).all(|l| l.split('\t').count() == 3));
    }

    #[test]
    fn normalize_ignores_case_space_and_quotes() {
        assert_eq!(normalize("  Don\u{2019}t   Worry. "), "don't worry");
    }
}
