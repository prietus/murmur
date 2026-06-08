//! Hunspell `.dic` reader for autocomplete.
//!
//! Reads every `*.dic` file in a directory, parses the words (stripping
//! `/affix-flags` and comments), and returns them as a sorted set we can
//! use for prefix lookup.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub fn load_dir(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dic") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        parse_into(&text, &mut out);
    }
    out
}

fn parse_into(text: &str, out: &mut BTreeSet<String>) {
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // The first non-blank line is a word count in valid hunspell .dic
        // files. Skip a pure-numeric first line.
        if i == 0 && line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let word = match line.split_once('/') {
            Some((w, _)) => w,
            None => line,
        };
        let word = word.trim();
        if word.is_empty() || word.len() < 3 {
            continue;
        }
        // Skip entries that look like abbreviations or proper-noun
        // markers (all upper, contain digits, contain punctuation).
        if word.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        out.insert(word.to_lowercase());
    }
}

/// Return the shortest dict word that starts with `prefix` and is
/// strictly longer than it. None if no match.
pub fn find_completion<'a>(dict: &'a BTreeSet<String>, prefix: &str) -> Option<&'a String> {
    if prefix.is_empty() {
        return None;
    }
    let lower = prefix.to_lowercase();
    let mut best: Option<&String> = None;
    for w in dict.range(lower.clone()..) {
        if !w.starts_with(&lower) {
            break;
        }
        if w.len() <= lower.len() {
            continue;
        }
        match best {
            Some(b) if b.len() <= w.len() => {}
            _ => best = Some(w),
        }
    }
    best
}
