//! Hunspell spell checking via `spellbook`.
//!
//! Unlike `dict` (which feeds autocomplete from raw `.dic` word lists),
//! real spell checking needs the matching `.aff` affix rules — the `.dic`
//! only carries lemmas (`habla/NS`), and inflected forms (`hablamos`)
//! come from affix expansion. We load every `.aff`+`.dic` pair found in
//! a directory and key the resulting checkers by ISO-639-1 lang code
//! derived from the filename stem (`es_ES` → `"es"`).

use spellbook::Dictionary;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Load every `.aff`+`.dic` pair in `dir` into the lang → checker map.
/// Pairs whose lang is already present are skipped (first one wins),
/// as are `.dic` files without a sibling `.aff`.
pub fn load_dir_into(dir: &Path, out: &mut HashMap<String, Dictionary>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("aff") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let lang: String = stem.chars().take(2).collect::<String>().to_ascii_lowercase();
        if lang.chars().count() < 2 || out.contains_key(&lang) {
            continue;
        }
        let dic_path = path.with_extension("dic");
        let (Ok(aff), Ok(dic)) = (fs::read_to_string(&path), fs::read_to_string(&dic_path))
        else {
            continue;
        };
        if let Ok(dict) = Dictionary::new(&aff, &dic) {
            out.insert(lang, dict);
        }
    }
}

/// Byte ranges of misspelled words in `input`, checked against `dict`.
///
/// Skips anything that isn't prose: URLs, words with digits, `#channel`
/// / `@nick` / `&chan` tokens, nicks present in `nicks`, and the final
/// word when the input doesn't end in whitespace (still being typed).
pub fn misspelled_ranges(
    dict: &Dictionary,
    input: &str,
    nicks: &[String],
) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let ends_mid_word = !input.ends_with(|c: char| c.is_whitespace());
    for (start, token) in split_tokens(input) {
        let end = start + token.len();
        // Don't flag the word under the cursor while it's being typed.
        if ends_mid_word && end == input.len() {
            continue;
        }
        if token.starts_with('#') || token.starts_with('@') || token.starts_with('&') {
            continue;
        }
        if token.contains("://") || token.starts_with("www.") {
            continue;
        }
        if token.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        // Trim punctuation off both edges; what's left is the word.
        let core = token.trim_matches(|c: char| !c.is_alphabetic());
        if core.chars().count() < 2 {
            continue;
        }
        if nicks.iter().any(|n| n.eq_ignore_ascii_case(core)) {
            continue;
        }
        if dict.check(core) {
            continue;
        }
        let core_start = start + (core.as_ptr() as usize - token.as_ptr() as usize);
        out.push((core_start, core_start + core.len()));
    }
    out
}

/// Whitespace-split with byte offsets.
fn split_tokens(s: &str) -> impl Iterator<Item = (usize, &str)> {
    s.split_whitespace()
        .map(move |tok| (tok.as_ptr() as usize - s.as_ptr() as usize, tok))
}
