//! Spaced-repetition drills built from your own logged mistakes (Leitner boxes).

use crate::actions::Mistake;
use crate::learn::{Entry, normalize};
use crate::paths;
use anyhow::Result;
use chrono::{Days, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Days until the next review for each box.
const INTERVALS: [u64; 7] = [1, 2, 4, 8, 16, 32, 64];

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub key: String,
    pub mistake: Mistake,
    /// How many times you made this mistake.
    pub seen: usize,
}

impl Card {
    /// The sentence to correct; falls back to the fragment when no context was logged.
    pub fn prompt(&self) -> &str {
        if self.mistake.context.is_empty() { &self.mistake.original } else { &self.mistake.context }
    }

    /// The context sentence with the mistake fixed, when the fragment can be found in it.
    pub fn corrected_sentence(&self) -> Option<String> {
        let context = &self.mistake.context;
        let start = context.find(&self.mistake.original)?;
        let mut out = context.clone();
        out.replace_range(start..start + self.mistake.original.len(), &self.mistake.fix);
        Some(out)
    }

    /// Accept the corrected fragment or the whole corrected sentence.
    pub fn check(&self, answer: &str) -> bool {
        let answer = normalize(answer);
        if answer.is_empty() {
            return false;
        }
        answer == normalize(&self.mistake.fix) || self.corrected_sentence().is_some_and(|s| normalize(&s) == answer)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CardState {
    pub level: usize,
    pub due: Option<NaiveDate>,
    pub reviews: u32,
    pub lapses: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Deck {
    pub cards: HashMap<String, CardState>,
}

fn deck_path() -> PathBuf {
    paths::data_dir().join("srs.json")
}

impl Deck {
    pub fn load() -> Deck {
        std::fs::read(deck_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        paths::write_private(&deck_path(), &serde_json::to_vec_pretty(self)?)
    }

    pub fn grade(&mut self, key: &str, correct: bool, today: NaiveDate) {
        let state = self.cards.entry(key.to_string()).or_default();
        state.reviews += 1;
        if correct {
            state.level = (state.level + 1).min(INTERVALS.len() - 1);
        } else {
            state.level = 0;
            state.lapses += 1;
        }
        state.due = today.checked_add_days(Days::new(INTERVALS[state.level]));
    }
}

pub fn card_key(m: &Mistake) -> String {
    format!("{}\u{1f}{}", normalize(&m.original), normalize(&m.fix))
}

/// One card per distinct mistake, keeping the newest context and explanation.
pub fn cards(entries: &[Entry]) -> Vec<Card> {
    let mut map: HashMap<String, Card> = HashMap::new();
    for m in entries.iter().flat_map(|e| &e.mistakes) {
        let key = card_key(m);
        let card = map.entry(key.clone()).or_insert_with(|| Card { key, mistake: m.clone(), seen: 0 });
        card.seen += 1;
        card.mistake = m.clone();
    }
    map.into_values().collect()
}

/// Cards to practise today: overdue reviews first, then new cards you repeat most.
pub fn due(entries: &[Entry], deck: &Deck, today: NaiveDate, limit: usize) -> Vec<Card> {
    let mut reviews = Vec::new();
    let mut new = Vec::new();
    for card in cards(entries) {
        match deck.cards.get(&card.key).and_then(|s| s.due) {
            Some(due) if due <= today => reviews.push((due, card)),
            Some(_) => {}
            None => new.push(card),
        }
    }
    reviews.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.seen.cmp(&a.1.seen)));
    new.sort_by(|a, b| b.seen.cmp(&a.seen).then(a.key.cmp(&b.key)));
    reviews.into_iter().map(|(_, c)| c).chain(new).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learn::tests::{entry, mistake};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
    }

    #[test]
    fn check_accepts_fragment_or_sentence() {
        let card = cards(&[entry(0, vec![mistake("informations", "information", "plural")])]).remove(0);
        assert!(card.check("Information"));
        assert!(card.check("i sent information yesterday"));
        assert!(!card.check("informations"));
        assert!(!card.check(""));
    }

    #[test]
    fn grading_moves_between_boxes() {
        let mut deck = Deck::default();
        deck.grade("k", true, today());
        assert_eq!(deck.cards["k"].due, Some(NaiveDate::from_ymd_opt(2026, 11, 22).unwrap()));
        deck.grade("k", false, today());
        assert_eq!(deck.cards["k"].level, 0);
        assert_eq!(deck.cards["k"].lapses, 1);
        assert_eq!(deck.cards["k"].due, Some(NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()));
    }

    #[test]
    fn due_orders_reviews_then_frequent_new_cards() {
        let a = mistake("a1", "b1", "article");
        let b = mistake("a2", "b2", "tense");
        let c = mistake("a3", "b3", "plural");
        let entries = vec![entry(1, vec![a.clone(), b.clone(), b.clone()]), entry(0, vec![c.clone()])];
        let mut deck = Deck::default();
        deck.cards.insert(card_key(&c), CardState { due: Some(today()), ..Default::default() });
        deck.cards.insert(
            card_key(&a),
            CardState { due: Some(NaiveDate::from_ymd_opt(2026, 12, 1).unwrap()), ..Default::default() },
        );
        let picked = due(&entries, &deck, today(), 10);
        let keys: Vec<&str> = picked.iter().map(|c| c.mistake.original.as_str()).collect();
        assert_eq!(keys, ["a3", "a2"]);
    }
}
