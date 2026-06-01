# Murmur

[murmur.priet.us](https://murmur.priet.us) — A native, opinionated IRC client. Built in Rust with [iced](https://iced.rs).

Murmur is a quiet client. Joins, parts, and noise fade into the background; conversation gets the foreground. Subtle by default, with `/dimm` for focus mode, inline media previews, tab-complete, command palette (⌘K), built-in emoji picker, per-channel logs, and SASL PLAIN / EXTERNAL (CertFP).

## Status

Alpha. Tested on macOS. Linux/Windows builds produced by CI but not heavily tested yet.

## Interface

- **Soft-dark** palette by default, plus `midnight`, `daylight`, and `solar` themes (`/theme <name>`).
- **Tile-style sidebar** with per-row hover/select animations (EaseOutBack 220ms) and channel-switch crossfade.
- **Sidebar / members panes** are toggleable with `‹` / `›` chevrons. Widths auto-fit the longest channel and nick.
- **Channel activity sparkline** — hover a channel for ≥5s to see a popup with a 2h sparkline (24 buckets × 5min) and a "N msgs · last Xmin" summary.
- **Per-network `&status` buffer** for server output, WHOIS replies, and standard replies.

## Reading

- **Message grouping** — consecutive messages from the same nick within 5 min collapse to a single header.
- **Subtle join/part lines** at ~35% opacity, smaller font, no nick column. `/hidejoins` removes them entirely for the current channel.
- **`/dimm`** (no args) — per-channel focus mode; non-mentions render at 30% opacity.
- **`/dimm <nick>`** — global soft-ignore at 15% opacity. `/ignore` is the hard equivalent.
- **Read-marker line** (`── new ──`) above the first unread when the channel was not focused. Syncs to the server via `draft/read-marker` so it survives across devices.
- **Smart unread digest** — when a backgrounded channel accumulates ≥5 unreads, the top of the buffer shows `↓ N new messages · M mentions · top: nick (X), nick (Y), nick (Z) · × dismiss` before the read marker.
- **Inline media previews** — images render inline (max 480×360); audio/video and other files as a sober card; URL errors as a faint footer.
- **Link unfurling** — OpenGraph / Twitter Cards / `<title>` fallback parsed and rendered as a card with title, description, host, and embedded image. `/unfurl on|off|toggle` per channel.
- **`+draft/reply` threading** — right-click → "↳ Reply" sets a composer chip; sent messages tag `+draft/reply`. Renders a mini-quote above the body, or a compact `↳` prefix when the parent is the message immediately above.
- **Reactions** — right-click → "React" or `/react <emoji>` adds a `+draft/react` TAGMSG; reactions render as grouped badges under the message.
- **In-buffer search** (⌘F) — case-insensitive substring with accent-tinted highlights and a match counter.

## Composer

- **Tab autocomplete** — nicks and slash commands; cycles on repeated tab. Smart `: ` vs ` ` suffix depending on position.
- **Command palette** (⌘K) — fuzzy channel jump + slash command runner.
- **Emoji picker** — searchable, curated set with category filters; inserts at cursor.
- **`draft/multiline`** — paste a multi-line block and send as one BATCH message.
- **`draft/typing`** — sends `+typing=active` while you type; observes inbound and renders "X is typing…" / "N others are typing…" above the input.
- **File upload** — paperclip button (or drag-and-drop) uploads via either the network's FILEHOST or a custom HTTP endpoint (configured under `[upload.custom]`).

## Sidebar — buddies & presence

- **`MONITOR`-backed buddy list** under the channel list. Online buddies get an accent dot; offline are dim; unknown render with a faint ring.
- **`/monitor add|del|list|clear`** (alias `/buddy`) — manage per-network buddies. Persisted to config.
- **Right-click any member → "Add buddy" / "Remove buddy"** for quick toggling.
- **Native notification** on offline → online transition (unless you're already viewing that DM).

## Search

- **`⌘F`** — in-buffer search overlay (current channel only).
- **`⌘⇧F`** — global FTS5 backlog search across every network and channel you've ever received a message in. Live-indexed via SQLite (`<config>/index.db`) with `unicode61` diacritic-insensitive tokenization (`sé` matches `se`). Snippet highlights, click → jump to that channel.

## Privacy & notes

- **`⌘⇧P`** — privacy mode. Replaces every nick in the UI with a stable mask (`u·a3f` derived from a session-independent hash, so threads stay readable). Affects chat nick column, members, buddies, DM names, reply quotes, and even nicks baked into system lines (`→ teraflops joined`) or referenced in message bodies (`nick: …`). Topics and your own message bodies are left as-is. An accent-tinted `PRIVACY` pill in the chat header tells you it's on.
- **Private notes per nick** — right-click any member → "Add note…" opens an inline editor. Saved notes show as a small accent dot on the member row and a tooltip with the full text when hovering. Stored per-network in your config TOML.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `⌘K` / `Ctrl+K` | Command palette (channel jump + slash commands) |
| `⌘F` / `Ctrl+F` | Search in current channel |
| `⌘⇧F` / `Ctrl+Shift+F` | Global backlog search |
| `⌘⇧P` / `Ctrl+Shift+P` | Toggle privacy mode |
| `⌘,` / `Ctrl+,` | Open settings |
| `⌘W` / `Ctrl+W` | Close current buffer |
| `Tab` | Autocomplete nick or slash command |
| `↑` / `↓` | Walk input history |
| `Esc` | Close the top-most overlay (search, palette, settings, picker, reply draft) |

## Slash commands

`/join /part /nick /me /msg /query /away /back /whois /topic /clear /kick /mode /op /deop /voice /devoice /ban /unban /invite /ignore /unignore /ignores /dimm /hidejoins /setname /history /delete /react /monitor /unfurl /raw (alias /quote) /theme /config /settings /server /connect /disconnect /close /logs /msgid /caps /ping /ctcp`

Run `⌘K` then `/` for one-line hints on each.

## IRCv3 support

Murmur attempts to establish the following IRCv3 capabilities with the server while connecting:

**Identity & presence**

- `account-tag` — every message carries the sender's services account
- `extended-join` — JOIN lines show services account when present
- `account-notify` — see when other users log in/out of NickServ
- `away-notify` — away users render dimmed in the member list
- `chghost` — host/ident changes shown in place, no fake quit/join
- `echo-message` — your own messages get a server-assigned `msgid` for editing/reacting
- `setname` — change your realname mid-session with `/setname <new realname>`

**Member list enrichment**

- `multi-prefix` — every channel prefix shown (`@+nick`, not just the highest)
- `userhost-in-names` — `ident@host` captured from NAMES for tooltips/whois

**Protocol plumbing**

- `message-tags` + `server-time` + `batch` — IRCv3 message metadata + tagged batches
- `invite-notify` — channel ops see who's being invited
- `cap-notify` — react to `CAP NEW` / `CAP DEL` mid-session (bouncer network attach/detach)
- `labeled-response` — response correlation for parallel commands
- `sts` — Strict Transport Security, persists per host; forces TLS+port on next connect

**Authentication**

- SASL `PLAIN` (password)
- SASL `EXTERNAL` (CertFP — auto-selected when `client_cert_path` is set)
- `draft/sasl-ir` — initial response on the first `AUTHENTICATE` line, skips the `+` challenge roundtrip

**History & catch-up**

- `draft/chathistory` — server-side scrollback on channel attach (`LATEST`)
- `draft/chathistory` `TARGETS` subcommand — `/history` shows active conversations
- `draft/event-playback` — soju/bouncer replays joins/parts/nicks alongside PRIVMSGs in chathistory
- IRCv3 standard replies (`FAIL` / `WARN` / `NOTE`) — rendered uniformly in the status buffer
- `RPL_ISUPPORT` (005) parser — uses `MODES=` for op-bulk chunking, `CHANTYPES=` for `/join` validation

**Modern drafts**

- `draft/typing` — "X is typing…" indicator below the input; sends `+typing=active` while you type
- `draft/multiline` — receives BATCH multiline as a single message with line breaks
- `draft/read-marker` — sends `MARKREAD` when you focus a channel
- `draft/message-redaction` — `/delete` removes your last message (or `/delete <msgid>`); incoming REDACTs render as tombstones
- `+draft/react` — `/react <emoji>` reacts to the latest message; reactions render as grouped badges

Server support varies — Ergo and Soju cover the most ground; Libera supports identity/presence/history but not the editing/redaction drafts.

## Build from source

```sh
cargo build --release
```

On Linux you'll need a few system packages. E.g., on Debian (and derivatives):

```sh
sudo apt-get install -y libxkbcommon-dev libwayland-dev libxkbcommon-x11-dev pkg-config
```

The first run writes a commented config template to:

- macOS: `~/Library/Application Support/murmur/config.toml`
- Linux: `~/.config/murmur/config.toml`
- Windows: `%APPDATA%\murmur\config.toml`

Edit `nickname` and `server`, restart, and you're in!

## Pre-built binaries

See the [Releases](../../releases) page.

## License

MIT — see [LICENSE](LICENSE).
