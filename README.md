# Murmur

[murmur.priet.us](https://murmur.priet.us) — A native, opinionated IRC client. Built in Rust with [iced](https://iced.rs).

Murmur is a quiet client that puts the conversation into the foreground. Joins, parts, and all other possible noise fade to the background instead. Murmur is subtle by default, but full of features. It provides a special focus mode (`\dimm`), inline media previews, tab-completion, a command palette (⌘K), a built-in emoji picker, per-channel logs, and supports SASL PLAIN / EXTERNAL (CertFP).

## Status

Alpha. Tested on macOS. Linux/Windows builds produced by CI but not heavily tested yet.

## IRCv3 support

Murmur attempts to establish the following IRCv3 capabilities with the server while connecting:

**Identity & presence**

- `account-tag` — causes the server to add a message tag containing the command sender’s services account
- `extended-join` — JOIN lines show services account when present
- `account-notify` — see when other users log in/out of NickServ
- `away-notify` — away users grayed out in the member list
- `chghost` — host/ident changes shown in place, no fake quit/join
- `echo-message` — your own messages get a server-assigned `msgid` for editing/reacting

**Member list enrichment**

- `multi-prefix` — every channel prefix shown (`@+nick`, not just the highest)
- `userhost-in-names` — `ident@host` captured from NAMES for tooltips/whois

**Protocol plumbing**

- `message-tags` + `server-time` + `batch` — IRCv3 message metadata + tagged batches
- `invite-notify` — channel ops see who's being invited
- `labeled-response` — response correlation for parallel commands
- `sts` — Strict Transport Security, persists per host; forces TLS+port on next connect

**Authentication**

- SASL `PLAIN` (password)
- SASL `EXTERNAL` (CertFP — auto-selected when `client_cert_path` is set)

**History & catch-up**

- `draft/chathistory` — server-side scrollback on channel attach (`LATEST`)
- `draft/chathistory` `TARGETS` subcommand — `/history` shows active conversations
- IRCv3 standard replies (`FAIL` / `WARN` / `NOTE`) — rendered uniformly in the status buffer
- `RPL_ISUPPORT` (005) parser — uses `MODES=` for op-bulk chunking, `CHANTYPES=` for `/join` validation

**Modern drafts**

- `draft/typing` — "X is typing…" indicator below the input; sends `+typing=active` while you type
- `draft/multiline` — receives BATCH multiline as a single message with line breaks
- `draft/read-marker` — sends `MARKREAD` when you focus a channel
- `draft/message-redaction` — `/delete` removes your last message (or `/delete <msgid>`); incoming REDACTs render as tombstones
- `+draft/react` — `/react <emoji>` reacts to the latest message; reactions render as grouped badges

Server support varies — Ergo and Soju cover the most ground; Libera supports identity/presence/history but not editing drafts.

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
