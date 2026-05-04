use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// New top-level config: networks list + global UI prefs.
// The legacy single-server format (flat fields at root) is migrated
// transparently on load — see `migrate_legacy_text`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default, rename = "network")]
    pub networks: Vec<NetworkConfig>,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub font_size_scale: Option<f32>,
}

// Per-network identity + connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub name: String,
    pub nickname: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub realname: Option<String>,
    pub server: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_tls")]
    pub use_tls: bool,
    #[serde(default)]
    pub nick_password: Option<String>,
    #[serde(default)]
    pub sasl_username: Option<String>,
    #[serde(default)]
    pub sasl_password: Option<String>,
    #[serde(default)]
    pub client_cert_path: Option<String>,
    #[serde(default)]
    pub client_cert_pass: Option<String>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default = "default_true")]
    pub autoconnect: bool,
}

impl std::hash::Hash for NetworkConfig {
    // Excludes `name` and `autoconnect`: rename shouldn't reconnect, and
    // autoconnect toggling is handled outside the subscription identity.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.nickname.hash(state);
        self.username.hash(state);
        self.realname.hash(state);
        self.server.hash(state);
        self.port.hash(state);
        self.use_tls.hash(state);
        self.nick_password.hash(state);
        self.sasl_username.hash(state);
        self.sasl_password.hash(state);
        self.client_cert_path.hash(state);
        self.client_cert_pass.hash(state);
        self.channels.hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None,
    NickServ,
    SaslPlain,
    SaslExternal,
}

impl NetworkConfig {
    pub fn auth_mode(&self) -> AuthMode {
        if self.client_cert_path.as_deref().is_some_and(|s| !s.is_empty()) {
            AuthMode::SaslExternal
        } else if self.sasl_password.as_deref().is_some_and(|s| !s.is_empty()) {
            AuthMode::SaslPlain
        } else if self.nick_password.as_deref().is_some_and(|s| !s.is_empty()) {
            AuthMode::NickServ
        } else {
            AuthMode::None
        }
    }

    pub fn sasl_user(&self) -> &str {
        self.sasl_username.as_deref().filter(|s| !s.is_empty()).unwrap_or(&self.nickname)
    }
}

// Legacy single-server flat schema. Used only during migration.
#[derive(Debug, Clone, Deserialize)]
struct LegacyAppConfig {
    nickname: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    realname: Option<String>,
    server: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_tls")]
    use_tls: bool,
    #[serde(default)]
    nick_password: Option<String>,
    #[serde(default)]
    sasl_username: Option<String>,
    #[serde(default)]
    sasl_password: Option<String>,
    #[serde(default)]
    client_cert_path: Option<String>,
    #[serde(default)]
    client_cert_pass: Option<String>,
    #[serde(default)]
    channels: Vec<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    font_family: Option<String>,
    #[serde(default)]
    font_size_scale: Option<f32>,
}

impl LegacyAppConfig {
    fn into_app_config(self) -> AppConfig {
        let name = derive_network_name(&self.server);
        let net = NetworkConfig {
            name,
            nickname: self.nickname,
            username: self.username,
            realname: self.realname,
            server: self.server,
            port: self.port,
            use_tls: self.use_tls,
            nick_password: self.nick_password,
            sasl_username: self.sasl_username,
            sasl_password: self.sasl_password,
            client_cert_path: self.client_cert_path,
            client_cert_pass: self.client_cert_pass,
            channels: self.channels,
            autoconnect: true,
        };
        AppConfig {
            networks: vec![net],
            theme: self.theme,
            font_family: self.font_family,
            font_size_scale: self.font_size_scale,
        }
    }
}

// Heuristic: irc.libera.chat → "libera", irc.oftc.net → "oftc".
// For unusual inputs (bare IP, single label) we keep the full string
// so the user has something recognizable.
pub fn derive_network_name(server: &str) -> String {
    let lower = server.trim().to_lowercase();
    if lower.is_empty() {
        return "network".into();
    }
    let labels: Vec<&str> = lower.split('.').filter(|s| !s.is_empty()).collect();
    if labels.len() < 2 {
        return lower;
    }
    let core = labels
        .iter()
        .copied()
        .find(|l| !matches!(*l, "irc" | "chat" | "ircs"))
        .unwrap_or(labels[0]);
    core.to_string()
}

fn default_port() -> u16 {
    6697
}
fn default_tls() -> bool {
    true
}
fn default_true() -> bool {
    true
}

pub enum LoadResult {
    Loaded(AppConfig),
    Migrated { cfg: AppConfig, backup: PathBuf },
    WroteTemplate(PathBuf),
    Error(String),
}

pub fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "murmur")
        .map(|p| p.config_dir().join("config.toml"))
}

pub fn load() -> LoadResult {
    let Some(path) = config_path() else {
        return LoadResult::Error("could not resolve config directory".into());
    };

    if !path.exists() {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return LoadResult::Error(format!("create {}: {e}", parent.display()));
            }
        }
        if let Err(e) = std::fs::write(&path, TEMPLATE) {
            return LoadResult::Error(format!("write {}: {e}", path.display()));
        }
        return LoadResult::WroteTemplate(path);
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => return LoadResult::Error(format!("read {}: {e}", path.display())),
    };

    // Prefer the new schema if it parses *and* declares at least one
    // network. An empty-networks parse is ambiguous (could be either a
    // brand-new file or a legacy one ignoring its flat fields), so we
    // fall through to legacy detection.
    if let Ok(cfg) = toml::from_str::<AppConfig>(&text) {
        if !cfg.networks.is_empty() {
            return LoadResult::Loaded(cfg);
        }
    }

    // Legacy single-server format: must have at least `nickname` and
    // `server` at the root.
    match toml::from_str::<LegacyAppConfig>(&text) {
        Ok(legacy) => {
            let backup = match write_backup(&path, &text) {
                Ok(p) => p,
                Err(e) => return LoadResult::Error(format!("backup: {e}")),
            };
            let cfg = legacy.into_app_config();
            let rendered = render_migrated_toml(&cfg);
            if let Err(e) = atomic_write(&path, &rendered) {
                return LoadResult::Error(format!("write migrated config: {e}"));
            }
            LoadResult::Migrated { cfg, backup }
        }
        Err(e) => LoadResult::Error(format!("parse {}: {e}", path.display())),
    }
}

fn write_backup(path: &Path, text: &str) -> std::io::Result<PathBuf> {
    let backup = path.with_extension("toml.bak");
    std::fs::write(&backup, text)?;
    Ok(backup)
}

fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn render_migrated_toml(cfg: &AppConfig) -> String {
    let header = format!(
        "# Migrated from single-server config on {}\n\
         # ====================================================================\n\
         # Murmur now supports connecting to multiple IRC networks at once.\n\
         # Each entry below is a [[network]] table with its own identity and\n\
         # auto-join channels. Add more entries to connect to more networks.\n\
         # The original file is preserved next to this one as config.toml.bak.\n\n",
        iso_today()
    );
    let body = toml::to_string_pretty(cfg).unwrap_or_else(|e| {
        // Should never happen with well-formed Serialize impls, but if
        // it does we degrade gracefully — the user still has their .bak.
        format!("# serialization error: {e}\n")
    });
    format!("{header}{body}")
}

fn iso_today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = now.div_euclid(86400);
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    days += 719468;
    let era = days.div_euclid(146097);
    let doe = days.rem_euclid(146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y as i32, m, d)
}

pub fn write_full_template_next_to_config() -> Result<PathBuf, String> {
    let Some(path) = config_path() else {
        return Err("could not resolve config directory".into());
    };
    let Some(parent) = path.parent() else {
        return Err("config has no parent directory".into());
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        return Err(format!("create {}: {e}", parent.display()));
    }
    let target = parent.join("config.template.toml");
    if let Err(e) = std::fs::write(&target, TEMPLATE) {
        return Err(format!("write {}: {e}", target.display()));
    }
    Ok(target)
}

// The TEMPLATE is the canonical reference for every option the
// client supports.
const TEMPLATE: &str = r##"# Murmur config
# ====================================================================
# Multi-network configuration. Each [[network]] block is a separate
# IRC connection, with its own identity and auto-join channels.
# Add as many [[network]] entries as you need.
#
# Some changes apply live (theme); others need a restart (font, server).
# To regenerate this template alongside your config any time, run
# /config template inside the app — it writes config.template.toml
# without touching your existing config.toml.

# === A network =============================================================

[[network]]
# Display label shown on the rail and in /server commands. Must be unique.
name = "libera"
nickname = "YOUR_NICK"
# username = "your_handle"     # default: same as nickname
# realname = "Your Name"       # default: same as nickname
server = "irc.libera.chat"
# port = 6697                   # default: 6697 (TLS)
# use_tls = true                # default: true
channels = ["#rust"]
# autoconnect = true            # default: true; set false to skip on startup

# === Authentication (per network) =========================================
# Pick AT MOST ONE. Priority if multiple are set:
#   SASL EXTERNAL  >  SASL PLAIN  >  NickServ IDENTIFY
#
# (1) NickServ IDENTIFY — legacy. Works only if the network grants a
#     grace period before forcing your nick away.
# nick_password = "your-nickserv-password"
#
# (2) SASL PLAIN — authenticates during connection. Required by some
#     networks and bouncers. sasl_username defaults to your nickname.
# sasl_username = "your-account"
# sasl_password = "your-password"
#
# (3) SASL EXTERNAL / CertFP — TLS client certificate auth.
#     Generate the PKCS#12 with:
#         openssl pkcs12 -export \
#             -inkey key.pem -in cert.pem \
#             -out client.p12 -passout pass:something-non-empty
#     Then attach the fingerprint to your account:
#         /msg NickServ CERT ADD <sha512-of-cert-DER>
#     macOS NOTE: client_cert_pass MUST NOT be empty — Apple's
#     Security framework rejects empty passphrases on PKCS#12.
# client_cert_path = "/absolute/path/to/client.p12"
# client_cert_pass = "something-non-empty"

# Add more networks below — copy a [[network]] block, change the name
# and server. Each network can have its own nick and credentials.
#
# [[network]]
# name = "oftc"
# nickname = "YOUR_NICK"
# server = "irc.oftc.net"
# channels = ["#debian"]

# === Appearance (global) ===================================================
#
# Theme — switch live in-app with /theme <name>. Available:
#   "soft-dark"  (default)  dark with a blue-grey tint
#   "midnight"              deeper dark blue
#   "daylight"              light theme
#   "solar"                 warm dark with orange accent
# theme = "soft-dark"
#
# Font family — any installed family. Falls back to the bundled
# JetBrains Mono if the chosen family isn't installed.
# font_family = "JetBrains Mono"
#
# Font size scale — multiplier on every UI text size (range 0.5..3.0).
# font_size_scale = 1.0
"##;
