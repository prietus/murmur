# Murmur

A native, opinionated IRC client. Built in Rust with [iced](https://iced.rs).

Murmur is a quiet client. Joins, parts, and noise fade into the background; conversation gets the foreground. Subtle by default, with `/dimm` for focus mode, inline media previews, tab-complete, command palette (⌘K), per-channel logs, and SASL PLAIN / EXTERNAL (CertFP).

## Status

Alpha. Tested on macOS. Linux/Windows builds produced by CI but not heavily exercised yet.

## Build from source

```sh
cargo build --release
```

On Linux you'll need a few system packages:

```sh
sudo apt-get install -y libxkbcommon-dev libwayland-dev libxkbcommon-x11-dev pkg-config
```

The first run writes a commented config template to:

- macOS: `~/Library/Application Support/murmur/config.toml`
- Linux: `~/.config/murmur/config.toml`
- Windows: `%APPDATA%\murmur\config.toml`

Edit `nickname` and `server`, restart, you're in.

## Pre-built binaries

See the [Releases](../../releases) page.

## License

MIT — see [LICENSE](LICENSE).
