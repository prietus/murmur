//! Full-text search over the local chatlog. Uses SQLite FTS5 stored at
//! `<config>/index.db`. Indexing is live-only: chat lines are inserted on
//! every PRIVMSG / ACTION via [`append`]. Backlog files written before
//! the FTS module was introduced are not auto-imported (see /reindex
//! when shipped).

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

fn db_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "murmur").map(|p| {
        p.config_dir().join("index.db")
    })
}

fn open() -> Option<Mutex<Connection>> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(&path).ok()?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    // FTS5 virtual table. `tokenize='unicode61 remove_diacritics 2'` so
    // search ignores accents (común vs comun).
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS msg_fts USING fts5(\
            ts UNINDEXED, \
            network UNINDEXED, \
            channel UNINDEXED, \
            nick, \
            body, \
            tokenize='unicode61 remove_diacritics 2'\
         );",
    )
    .ok()?;
    Some(Mutex::new(conn))
}

fn with_db<R>(f: impl FnOnce(&Connection) -> R) -> Option<R> {
    let m = DB.get_or_init(|| open().expect("could not open fts db"));
    let guard = m.lock().ok()?;
    Some(f(&guard))
}

pub fn append(network: &str, channel: &str, nick: &str, body: &str, ts_iso: &str) {
    if channel.is_empty() || channel.starts_with('&') || body.is_empty() {
        return;
    }
    with_db(|conn| {
        let _ = conn.execute(
            "INSERT INTO msg_fts(ts, network, channel, nick, body) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![ts_iso, network, channel, nick, body],
        );
    });
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub ts: String,
    pub network: String,
    pub channel: String,
    pub nick: String,
    pub body: String,
    /// FTS5 snippet with `‹…›` around matches (truncated, ~32 tokens).
    pub snippet: String,
}

pub fn search(query: &str, limit: usize) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let match_q = sanitize_fts_query(q);
    if match_q.is_empty() {
        return Vec::new();
    }
    with_db(|conn| {
        let mut stmt = match conn.prepare(
            "SELECT ts, network, channel, nick, body, \
                    snippet(msg_fts, 4, '‹', '›', '…', 32) \
             FROM msg_fts \
             WHERE msg_fts MATCH ?1 \
             ORDER BY rowid DESC \
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map(params![match_q, limit as i64], |r| {
            Ok(Hit {
                ts: r.get::<_, String>(0).unwrap_or_default(),
                network: r.get::<_, String>(1).unwrap_or_default(),
                channel: r.get::<_, String>(2).unwrap_or_default(),
                nick: r.get::<_, String>(3).unwrap_or_default(),
                body: r.get::<_, String>(4).unwrap_or_default(),
                snippet: r.get::<_, String>(5).unwrap_or_default(),
            })
        });
        match rows {
            Ok(it) => it.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    })
    .unwrap_or_default()
}

/// Turn a user query into an FTS5 MATCH expression. Each whitespace token
/// becomes a prefix term (`foo*`); FTS5 syntax chars are stripped so the
/// user can paste arbitrary text without triggering query errors.
fn sanitize_fts_query(q: &str) -> String {
    let cleaned: String = q
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned
        .split_whitespace()
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" ")
}
