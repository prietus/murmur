//! Hunspell `.dic` reader for autocomplete.
//!
//! Reads every `*.dic` file in a directory, parses the words (stripping
//! `/affix-flags` and comments), and groups them by ISO-639-1 language
//! code derived from the filename. `en_US.dic` and `en_GB.dic` both
//! merge into the `"en"` bucket.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

/// Load every `*.dic` in `dir` and merge into the given lang → words
/// map. Lang code = lowercase first 2 letters of the filename stem.
pub fn load_dir_into(dir: &Path, out: &mut HashMap<String, BTreeSet<String>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("dic") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let lang: String = stem.chars().take(2).collect::<String>().to_ascii_lowercase();
        if lang.chars().count() < 2 {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let bucket = out.entry(lang).or_default();
        parse_into(&text, bucket);
    }
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
