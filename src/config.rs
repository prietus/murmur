use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
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
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub font_size_scale: Option<f32>,
}

impl std::hash::Hash for AppConfig {
    // Subscription identity — only fields that affect the connection
    // matter. Theme/font/scale changes shouldn't reconnect.
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

impl AppConfig {
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

fn default_port() -> u16 {
    6697
}
fn default_tls() -> bool {
    true
}

pub enum LoadResult {
    Loaded(AppConfig),
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
    match toml::from_str::<AppConfig>(&text) {
        Ok(cfg) => LoadResult::Loaded(cfg),
        Err(e) => LoadResult::Error(format!("parse {}: {e}", path.display())),
    }
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
// client supports. The bare-minimum required keys (`nickname`,
// `server`) are uncommented; everything else ships commented with
// the default value shown so a reader can copy/paste and tweak.
const TEMPLATE: &str = r##"# IRC client config
# ====================================================================
# This file documents every option. Required keys are uncommented;
# everything else shows its default and is commented out — uncomment
# the ones you want to override.
#
# Some changes apply live (theme); others need a restart (font, server).
# To regenerate this template alongside your config any time, run
# /config template inside the app — it writes config.template.toml
# without touching your existing config.toml.

# === Identity ============================================================

nickname = "YOUR_NICK"
# username = "your_handle"     # default: same as nickname
# realname = "Your Name"       # default: same as nickname

# === Server ==============================================================

server = "irc.libera.chat"
# port = 6697                   # default: 6697 (TLS)
# use_tls = true                # default: true

# === Channels to auto-join ===============================================

channels = ["#rust"]

# === Authentication ======================================================
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

# === Appearance ==========================================================
#
# Theme — switch live in-app with /theme <name>. Available:
#   "soft-dark"  (default)  dark with a blue-grey tint
#   "midnight"              deeper dark blue
#   "daylight"              light theme
#   "solar"                 warm dark with orange accent
# theme = "soft-dark"
#
# Font family — any installed family. Falls back to the bundled
# JetBrains Mono if the chosen family isn't installed. List your
# installed families on macOS with:
#   system_profiler SPFontsDataType 2>/dev/null \
#       | awk -F': ' '/Family:/ {print $2}' | sort -u
# font_family = "JetBrains Mono"
#
# Font size scale — multiplier on every UI text size (range 0.5..3.0).
# Base sizes are in logical pixels (chat body = 13). Examples:
#   1.0   default
#   1.15  ~+15% (chat body becomes ~15 px)
#   0.9   ~-10%
# Retina/HiDPI is handled by the OS — this knob only changes logical
# pixel sizes, independent of display density.
# font_size_scale = 1.0
"##;
