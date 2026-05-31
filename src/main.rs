#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod emoji;
mod irc_worker;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::path::PathBuf;
use std::time::Instant;

use std::pin::Pin;

use futures::channel::mpsc;
use futures::Stream;
use iced::animation::{Animation, Easing};
use iced::keyboard;
use iced::widget::{
    button, checkbox, column, container, image as iced_image, mouse_area, pick_list, radio, row,
    scrollable, slider, stack, text, text_input, Space,
};
use iced::ContentFit;
use iced::{
    window, Background, Border, Color, Element, Fill, Font, Length, Padding, Shadow, Subscription,
    Task, Theme,
};

use crate::config::{AppConfig, LoadResult, NetworkConfig};
use crate::irc_worker::{Event as IrcEvent, Outgoing};

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const FONT_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
const FONT_NAME: &str = "JetBrains Mono";

static USER_FONT: OnceLock<&'static str> = OnceLock::new();
static FONT_SCALE: OnceLock<f32> = OnceLock::new();
// Local UTC offset in seconds, captured once at process start before any
// threads spawn. IRCv3 `server-time` is always UTC; we add this to render
// timestamps in the user's local zone. DST changes during a long-running
// session won't update until relaunch.
static LOCAL_OFFSET_SECS: AtomicI32 = AtomicI32::new(0);

pub fn local_offset_secs() -> i64 {
    LOCAL_OFFSET_SECS.load(Ordering::Relaxed) as i64
}

const FADE_MS: u128 = 250;
const GROUP_SECS: u64 = 300;

const TIME_W: u32 = 44;
const NICK_W: u32 = 92;

const SIDEBAR_MIN_W: f32 = 130.0;
const SIDEBAR_MAX_W: f32 = 240.0;
const MEMBERS_MIN_W: f32 = 110.0;
const MEMBERS_MAX_W: f32 = 200.0;

const PALETTE_INPUT_ID: &str = "palette-input";
const COMPOSE_INPUT_ID: &str = "compose-input";
const SEARCH_INPUT_ID: &str = "search-input";
const EMOJI_PICKER_INPUT_ID: &str = "emoji-picker-input";
const PALETTE_W: f32 = 520.0;
const PALETTE_MAX_ITEMS: usize = 8;
const EMOJI_CELL: u32 = 34;

const PALETTE_COMMANDS: &[(&str, &str, bool)] = &[
    ("/dimm", "focus mode, or soft-ignore a nick", false),
    ("/join", "join a channel", true),
    ("/part", "leave current channel", false),
    ("/nick", "change your nick", true),
    ("/me", "send an action", true),
    ("/msg", "send a private message", true),
    ("/query", "open a DM with a nick", true),
    ("/theme", "switch theme: soft-dark | midnight | daylight | solar", true),
    ("/config", "show config path, or `/config template` to regenerate reference", false),
    ("/ctcp", "send a CTCP query: /ctcp <nick> <COMMAND> [args]", true),
    ("/ping", "ping a user: /ping <nick>", true),
    ("/hidejoins", "toggle hiding join/part lines in the current channel", false),
    ("/logs", "show the chat log directory path", false),
    ("/server", "switch active network: /server <name>", true),
    ("/connect", "enable autoconnect for a network: /connect <name>", true),
    ("/disconnect", "disconnect from a network (current if no name)", false),
    ("/settings", "open the settings panel", false),
    ("/close", "close current buffer (DM/channel) — alias /wc, /q", false),
    ("/ignore", "hide all messages from a nick (persisted) — no args to list", false),
    ("/unignore", "remove a nick from the ignore list", true),
    ("/ignores", "list ignored nicks", false),
    ("/away", "set away with optional reason — /back to clear", false),
    ("/back", "clear away status", false),
    ("/whois", "WHOIS a nick — reply in the status buffer (alias /wi)", true),
    ("/topic", "show or set the current channel's topic", false),
    ("/clear", "clear messages in the current buffer (logs unaffected)", false),
    ("/raw", "send a raw IRC line — alias /quote", true),
    ("/kick", "kick a user from the current channel: /kick <nick> [reason]", true),
    ("/mode", "set or query modes: /mode <target> [<modes> [args]]", true),
    ("/op", "grant operator (+o) to one or more nicks on current channel", true),
    ("/deop", "remove operator (-o) from one or more nicks", true),
    ("/voice", "grant voice (+v) to one or more nicks", true),
    ("/devoice", "remove voice (-v) from one or more nicks", true),
    ("/ban", "ban a nick or mask on the current channel (+b)", true),
    ("/unban", "lift a ban (-b) on the current channel", true),
    ("/invite", "invite a nick to a channel: /invite <nick> [#chan]", true),
    ("/history", "list targets with chathistory activity (last 7 days, max 50)", false),
    ("/delete", "delete your last message — or /delete <msgid> for a specific one", false),
    ("/react", "react with emoji: /react <emoji> (on your last received msg) or /react <msgid> <emoji>", true),
    ("/setname", "change your realname mid-session (IRCv3 setname): /setname <new realname>", true),
    ("/msgid", "debug: show msgid of last message — or /msgid <substring> to grep recent ones", false),
    ("/caps", "debug: list IRCv3 capabilities ACKed by the server", false),
];

fn main() -> iced::Result {
    // Must run before any other thread spawns: the `time` crate refuses to
    // read the local TZ from a multithreaded process (POSIX unsoundness).
    if let Ok(off) = time::UtcOffset::current_local_offset() {
        LOCAL_OFFSET_SECS.store(off.whole_seconds(), Ordering::Relaxed);
    }

    // macOS: pin the application identity used by notify-rust so its
    // underlying mac-notification-sys does not fall back to an in-process
    // AppleScript bundle-id lookup, which on unsigned Mach-O binaries can
    // pop a Script Editor / "open as script" prompt on the first call.
    // Outside a real .app bundle this is a no-op cosmetically but still
    // skips the AppleScript path.
    #[cfg(target_os = "macos")]
    {
        let _ = notify_rust::set_application("us.priet.murmur");
    }

    if let LoadResult::Loaded(cfg) = config::load() {
        if let Some(fam) = cfg.font_family.as_deref().filter(|s| !s.is_empty()) {
            let leaked: &'static str = Box::leak(fam.to_string().into_boxed_str());
            let _ = USER_FONT.set(leaked);
        }
        if let Some(scale) = cfg
            .font_size_scale
            .filter(|s| s.is_finite() && *s >= 0.5 && *s <= 3.0)
        {
            let _ = FONT_SCALE.set(scale);
        }
        if let Some(p) = cfg.theme.as_deref().and_then(themes::by_name) {
            theme::set(p);
        }
    }

    iced::application(App::default, App::update, App::view)
        .title("Murmur")
        .theme(App::theme)
        .subscription(App::subscription)
        .default_font(Font::with_name(user_font_name()))
        .font(FONT_REGULAR)
        .font(FONT_MEDIUM)
        .run()
}

mod tok {
    pub const S1: f32 = 3.0;
    pub const S2: f32 = 6.0;
    pub const S3: f32 = 10.0;
    pub const S4: f32 = 12.0;
    #[allow(dead_code)]
    pub const S5: f32 = 16.0;
    #[allow(dead_code)]
    pub const S6: f32 = 18.0;

    use iced::Color;
    use crate::theme;

    pub fn bg_0() -> Color { theme::current().bg_0 }
    pub fn bg_1() -> Color { theme::current().bg_1 }
    pub fn bg_2() -> Color { theme::current().bg_2 }
    pub fn bg_hover() -> Color { theme::current().bg_hover }
    pub fn bg_elev() -> Color { theme::current().bg_elev }
    pub fn border() -> Color { theme::current().border }
    pub fn border_soft() -> Color { theme::current().border_soft }
    pub fn text() -> Color { theme::current().text }
    pub fn text_mid() -> Color { theme::current().text_mid }
    pub fn text_muted() -> Color { theme::current().text_muted }
    pub fn text_faint() -> Color { theme::current().text_faint }
    pub fn accent() -> Color { theme::current().accent }
    pub fn accent_soft() -> Color { theme::current().accent_soft }
    #[allow(dead_code)]
    pub fn accent_ring() -> Color { theme::current().accent_ring }
}

mod theme {
    use std::sync::RwLock;
    use crate::Palette;

    static CURRENT: RwLock<Palette> = RwLock::new(crate::themes::SOFT_DARK);

    pub fn current() -> Palette {
        *CURRENT.read().expect("theme rwlock poisoned")
    }

    pub fn set(p: Palette) {
        *CURRENT.write().expect("theme rwlock poisoned") = p;
    }
}

#[derive(Clone, Copy)]
struct Palette {
    bg_0: Color,
    bg_1: Color,
    bg_2: Color,
    bg_hover: Color,
    bg_elev: Color,
    border: Color,
    border_soft: Color,
    text: Color,
    text_mid: Color,
    text_muted: Color,
    text_faint: Color,
    accent: Color,
    accent_soft: Color,
    accent_ring: Color,
    is_dark: bool,
}

mod themes {
    use super::Palette;
    use iced::Color;

    pub const SOFT_DARK: Palette = Palette {
        bg_0: Color::from_rgb(0.086, 0.094, 0.118),
        bg_1: Color::from_rgb(0.110, 0.122, 0.153),
        bg_2: Color::from_rgb(0.141, 0.157, 0.196),
        bg_hover: Color::from_rgb(0.173, 0.196, 0.247),
        bg_elev: Color::from_rgb(0.149, 0.165, 0.204),
        border: Color::from_rgb(0.200, 0.220, 0.278),
        border_soft: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
        text: Color::from_rgb(0.945, 0.949, 0.965),
        text_mid: Color::from_rgb(0.706, 0.720, 0.761),
        text_muted: Color::from_rgb(0.533, 0.553, 0.612),
        text_faint: Color::from_rgb(0.396, 0.416, 0.471),
        accent: Color::from_rgb(0.478, 0.600, 1.0),
        accent_soft: Color::from_rgba(0.478, 0.600, 1.0, 0.14),
        accent_ring: Color::from_rgba(0.478, 0.600, 1.0, 0.40),
        is_dark: true,
    };

    pub const MIDNIGHT: Palette = Palette {
        bg_0: Color::from_rgb(0.043, 0.047, 0.063),
        bg_1: Color::from_rgb(0.063, 0.071, 0.090),
        bg_2: Color::from_rgb(0.086, 0.098, 0.122),
        bg_hover: Color::from_rgb(0.122, 0.137, 0.169),
        bg_elev: Color::from_rgb(0.094, 0.106, 0.133),
        border: Color::from_rgb(0.157, 0.176, 0.220),
        border_soft: Color::from_rgba(1.0, 1.0, 1.0, 0.05),
        text: Color::from_rgb(0.949, 0.961, 0.984),
        text_mid: Color::from_rgb(0.682, 0.702, 0.749),
        text_muted: Color::from_rgb(0.486, 0.514, 0.580),
        text_faint: Color::from_rgb(0.349, 0.376, 0.443),
        accent: Color::from_rgb(0.553, 0.788, 0.929),
        accent_soft: Color::from_rgba(0.553, 0.788, 0.929, 0.14),
        accent_ring: Color::from_rgba(0.553, 0.788, 0.929, 0.40),
        is_dark: true,
    };

    pub const DAYLIGHT: Palette = Palette {
        bg_0: Color::from_rgb(0.965, 0.969, 0.976),
        bg_1: Color::from_rgb(1.0, 1.0, 1.0),
        bg_2: Color::from_rgb(0.949, 0.953, 0.965),
        bg_hover: Color::from_rgb(0.918, 0.929, 0.949),
        bg_elev: Color::from_rgb(0.973, 0.976, 0.984),
        border: Color::from_rgb(0.831, 0.851, 0.886),
        border_soft: Color::from_rgba(0.0, 0.0, 0.0, 0.06),
        text: Color::from_rgb(0.090, 0.110, 0.149),
        text_mid: Color::from_rgb(0.298, 0.329, 0.388),
        text_muted: Color::from_rgb(0.443, 0.475, 0.541),
        text_faint: Color::from_rgb(0.580, 0.612, 0.671),
        accent: Color::from_rgb(0.196, 0.388, 0.847),
        accent_soft: Color::from_rgba(0.196, 0.388, 0.847, 0.10),
        accent_ring: Color::from_rgba(0.196, 0.388, 0.847, 0.35),
        is_dark: false,
    };

    pub const SOLAR: Palette = Palette {
        bg_0: Color::from_rgb(0.106, 0.082, 0.071),
        bg_1: Color::from_rgb(0.137, 0.110, 0.094),
        bg_2: Color::from_rgb(0.176, 0.137, 0.114),
        bg_hover: Color::from_rgb(0.227, 0.180, 0.149),
        bg_elev: Color::from_rgb(0.184, 0.149, 0.129),
        border: Color::from_rgb(0.290, 0.227, 0.184),
        border_soft: Color::from_rgba(1.0, 0.95, 0.88, 0.06),
        text: Color::from_rgb(0.961, 0.929, 0.851),
        text_mid: Color::from_rgb(0.776, 0.722, 0.620),
        text_muted: Color::from_rgb(0.580, 0.529, 0.451),
        text_faint: Color::from_rgb(0.420, 0.376, 0.314),
        accent: Color::from_rgb(0.984, 0.678, 0.290),
        accent_soft: Color::from_rgba(0.984, 0.678, 0.290, 0.14),
        accent_ring: Color::from_rgba(0.984, 0.678, 0.290, 0.40),
        is_dark: true,
    };

    pub fn by_name(name: &str) -> Option<Palette> {
        match name.to_lowercase().as_str() {
            "soft-dark" | "soft_dark" | "softdark" | "default" => Some(SOFT_DARK),
            "midnight" | "dark" => Some(MIDNIGHT),
            "daylight" | "light" => Some(DAYLIGHT),
            "solar" | "warm" => Some(SOLAR),
            _ => None,
        }
    }

    pub const ALL: &[(&str, Palette)] = &[
        ("soft-dark", SOFT_DARK),
        ("midnight", MIDNIGHT),
        ("daylight", DAYLIGHT),
        ("solar", SOLAR),
    ];
}

fn pad(top: f32, right: f32, bottom: f32, left: f32) -> Padding {
    Padding { top, right, bottom, left }
}

fn sp(w: impl Into<iced::Length>, h: impl Into<iced::Length>) -> Space {
    Space::new().width(w).height(h)
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let taken: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{taken}…")
    }
}

fn user_font_name() -> &'static str {
    USER_FONT.get().copied().unwrap_or(FONT_NAME)
}

fn font_scale() -> f32 {
    FONT_SCALE.get().copied().unwrap_or(1.0)
}

fn sz(base: f32) -> f32 {
    (base - 1.0).max(9.0) * font_scale()
}

fn regular() -> Font {
    Font::with_name(user_font_name())
}

fn medium() -> Font {
    Font {
        weight: iced::font::Weight::Medium,
        ..Font::with_name(user_font_name())
    }
}

fn italic() -> Font {
    Font {
        style: iced::font::Style::Italic,
        ..Font::with_name(user_font_name())
    }
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: (a.a + (b.a - a.a) * t).clamp(0.0, 1.0),
    }
}

fn channel_parts(name: &str) -> (&'static str, String) {
    if let Some(rest) = name.strip_prefix("##") {
        return ("#", rest.to_string());
    }
    if let Some(rest) = name.strip_prefix('#') {
        return ("#", rest.to_string());
    }
    if let Some(rest) = name.strip_prefix('&') {
        return ("•", rest.to_string());
    }
    ("@", name.to_string())
}

#[derive(Clone)]
enum Message {
    ChannelSelected(usize),
    InputChanged(String),
    SendMessage,
    ToggleSidebar,
    ToggleMembers,
    Tick(Instant),
    Irc(NetworkId, IrcEvent),
    Key(keyboard::Event),
    PaletteQuery(String),
    PaletteActivate,
    PaletteActivateIdx(usize),
    PaletteClose,
    HoverChannel(Option<usize>),
    HoverMember(Option<usize>),
    StartDmWith(String),
    MediaFetched(FetchedMedia),
    CloseChannel(usize),
    NetworkSelected(NetworkId),
    HoverNetwork(Option<NetworkId>),
    OpenUrl(String),
    WindowFocus(bool),
    SettingsClose,
    SettingsSelectSection(SettingsSection),
    SettingsThemeChanged(String),
    SettingsFontFamily(String),
    SettingsFontScale(f32),
    SettingsKwInput(String),
    SettingsKwAdd,
    SettingsKwRemove(usize),
    SettingsNetSelect(usize),
    SettingsNetAdd,
    SettingsNetRemove(usize),
    SettingsNetField(NetField, String),
    SettingsNetTls(bool),
    SettingsNetAutoconnect(bool),
    SettingsNetAuthMode(SettingsAuthMode),
    SettingsNetChannelInput(String),
    SettingsNetChannelAdd,
    SettingsNetChannelRemove(usize),
    SettingsSave,
    EmojiPickerToggle,
    EmojiPickerClose,
    EmojiPickerQuery(String),
    EmojiPickerCategory(emoji::Category),
    EmojiInsert(&'static str),
    MessageContextOpen { channel_idx: usize, msgid: String },
    MessageContextClose,
    MessageContextDelete,
    MessageContextStartReact,
    MemberContextOpen { nick: String },
    MemberContextClose,
    MemberContextDm,
    MemberContextWhois,
    MemberContextIgnoreToggle,
    MemberContextOpToggle,
    MemberContextVoiceToggle,
    MemberContextKick,
    MemberContextBan,
    AttachFilePressed,
    FilePicked(Option<std::path::PathBuf>),
    UploadFinished(Result<String, String>),
    FileHovered,
    FileDropped(std::path::PathBuf),
    FilesHoveredLeft,
    SettingsUploadUseCustom(bool),
    SettingsUploadField(UploadField, String),
    SettingsUploadKind(String),
    SearchClose,
    SearchQuery(String),
}

#[derive(Clone)]
struct FetchedMedia {
    url: String,
    state: MediaState,
}

#[derive(Clone)]
enum MediaState {
    Loading,
    Image { handle: iced_image::Handle, w: u32, h: u32 },
    File { kind: MediaKind, content_type: String, size: Option<u64> },
    LinkCard {
        title: Option<String>,
        description: Option<String>,
        host: String,
        image: Option<(iced_image::Handle, u32, u32)>,
    },
    Skipped,
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
}


#[derive(Clone)]
enum PaletteItem {
    Channel(usize),
    Command { name: &'static str, hint: &'static str, needs_args: bool },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Appearance,
    Notifications,
    Networks,
    Upload,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UploadField {
    Url,
    Token,
    Field,
    ResponseKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NetField {
    Name,
    Nickname,
    Username,
    Realname,
    Server,
    Port,
    NickPassword,
    SaslUsername,
    SaslPassword,
    ClientCertPath,
    ClientCertPass,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsAuthMode {
    None,
    NickServ,
    SaslPlain,
    SaslExternal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnStatus {
    NotConfigured,
    TemplateWritten,
    Connecting,
    Connected,
    Disconnected,
    Error,
}

#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
struct NetworkId(u32);

struct NetworkState {
    id: NetworkId,
    cfg: NetworkConfig,
    status: ConnStatus,
    outgoing: Option<mpsc::Sender<Outgoing>>,
    last_error: Option<String>,
    autoconnect_enabled: bool,
    last_selected: Option<usize>,
    caps_acked: HashSet<String>,
    /// Lowercased nicks currently marked away (from `away-notify`).
    away_nicks: HashSet<String>,
    /// Lowercased nick → services account (from `account-notify`
    /// + the `account` tag + `extended-join`).
    accounts: HashMap<String, String>,
    /// Cached ISUPPORT (005) features for this connection.
    isupport: irc_worker::ISupport,
}

struct App {
    networks: Vec<NetworkState>,
    active: Option<NetworkId>,
    channels: Vec<Channel>,
    selected: usize,
    input: String,
    now: Instant,
    sidebar_anim: Animation<bool>,
    members_anim: Animation<bool>,
    #[allow(dead_code)]
    cfg_path: Option<PathBuf>,
    fallback_status: ConnStatus,
    last_error: Option<String>,
    dimmed_nicks: HashSet<String>,
    ignored_nicks: HashSet<String>,
    palette_open: bool,
    palette_query: String,
    palette_cursor: usize,
    hovered_channel: Option<usize>,
    hovered_member: Option<usize>,
    hovered_network: Option<NetworkId>,
    tab_state: Option<TabState>,
    media_cache: HashMap<String, MediaState>,
    theme_name: String,
    input_history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    window_focused: bool,
    highlight_keywords: Vec<String>,
    settings_open: bool,
    settings_draft: AppConfig,
    settings_section: SettingsSection,
    settings_kw_input: String,
    settings_net_idx: usize,
    settings_net_channel_input: String,
    settings_save_error: Option<String>,
    settings_save_info: Option<String>,
    /// (network, bucket, nick) → last time we saw a `+typing=active` TAGMSG.
    /// Entries expire after 6 seconds without renewal (per spec).
    typing_observed: HashMap<(NetworkId, String, String), Instant>,
    /// When we last sent a `+typing=active` TAGMSG for a given (network, target).
    typing_sent: HashMap<(NetworkId, String), Instant>,
    /// Per-target server-side read marker (timestamp last reported by MARKREAD).
    read_markers: HashMap<(NetworkId, String), String>,
    emoji_picker: Option<EmojiPickerState>,
    message_context: Option<MessageContextState>,
    member_context: Option<MemberContextState>,
    /// True while a FILEHOST upload is in flight (disables the attach button).
    uploading: bool,
    /// Active file-upload backend config (server FILEHOST vs custom HTTP).
    upload_cfg: config::UploadConfig,
    /// True while a file is being dragged over the window. Drives the
    /// composer's drop-hint border.
    file_hover: bool,
    /// In-buffer search (⌘F). Highlights case-insensitive matches in
    /// the currently-selected channel's messages.
    search: Option<SearchState>,
}

#[derive(Default)]
struct SearchState {
    query: String,
}

#[derive(Default)]
struct EmojiPickerState {
    query: String,
    category: Option<emoji::Category>,
    react_target: Option<ReactTarget>,
}

#[derive(Clone)]
struct ReactTarget {
    channel_idx: usize,
    msgid: String,
}

struct MessageContextState {
    channel_idx: usize,
    msgid: String,
}

struct MemberContextState {
    channel_idx: usize,
    nick: String,
}

struct TabState {
    word_start: usize,
    matches: Vec<String>,
    idx: usize,
    suffix: &'static str,
    expected_input: String,
}

// Per-member metadata gathered from NAMES / JOIN with multi-prefix +
// userhost-in-names. Keyed (in the parallel map on Channel) by the
// member's nick exactly as it appears in `members`.
#[derive(Clone, Default)]
struct MemberMeta {
    /// Highest-priority channel prefix character (`~&@%+`), or empty.
    prefixes: String,
    userhost: Option<String>,
}

struct Channel {
    network_id: NetworkId,
    name: String,
    topic: Option<String>,
    messages: Vec<ChatMessage>,
    members: Vec<String>,
    /// Metadata for entries in `members`, keyed by nick.
    member_meta: HashMap<String, MemberMeta>,
    dimm: bool,
    hide_joinpart: bool,
    hover_anim: Animation<bool>,
    select_anim: Animation<bool>,
    fade_baseline: Instant,
    has_unread: bool,
    has_mention: bool,
    /// Whether we've already asked the server for initial backlog
    /// (CHATHISTORY LATEST). Set on first self-join to avoid refetching.
    chathistory_requested: bool,
    /// Index of the first unread message in `messages`. The read-marker
    /// separator is drawn just above this message. Set when a message
    /// arrives while the channel is not active+focused; cleared on
    /// switch-away or window blur (so the next batch of unreads gets a
    /// fresh anchor).
    read_marker_idx: Option<usize>,
}

fn new_row_anim() -> Animation<bool> {
    Animation::new(false)
        .duration(std::time::Duration::from_millis(220))
        .easing(Easing::EaseOutBack)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MsgKind {
    Chat,
    Action,
    System,
    JoinPart,
}

struct ChatMessage {
    nick: String,
    body: String,
    time: String,
    day: String,
    inserted_at: Instant,
    mono_secs: u64,
    kind: MsgKind,
    msgid: Option<String>,
    /// Emoji reactions on this message: emoji → set of reactor nicks.
    reactions: HashMap<String, HashSet<String>>,
}

impl Default for App {
    fn default() -> Self {
        let now = Instant::now();
        let base = App {
            networks: Vec::new(),
            active: None,
            channels: Vec::new(),
            selected: 0,
            input: String::new(),
            now,
            sidebar_anim: Animation::new(true).quick().easing(Easing::EaseOutQuint),
            members_anim: Animation::new(true).quick().easing(Easing::EaseOutQuint),
            cfg_path: config::config_path(),
            fallback_status: ConnStatus::NotConfigured,
            last_error: None,
            dimmed_nicks: HashSet::new(),
            ignored_nicks: HashSet::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_cursor: 0,
            hovered_channel: None,
            hovered_member: None,
            hovered_network: None,
            tab_state: None,
            media_cache: HashMap::new(),
            theme_name: "soft-dark".into(),
            input_history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            window_focused: true,
            highlight_keywords: Vec::new(),
            settings_open: false,
            settings_draft: AppConfig {
                networks: Vec::new(),
                theme: None,
                font_family: None,
                font_size_scale: None,
                highlight_keywords: Vec::new(),
                ignored_nicks: Vec::new(),
                upload: config::UploadConfig::default(),
            },
            settings_section: SettingsSection::Appearance,
            settings_kw_input: String::new(),
            settings_net_idx: 0,
            settings_net_channel_input: String::new(),
            settings_save_error: None,
            settings_save_info: None,
            typing_observed: HashMap::new(),
            typing_sent: HashMap::new(),
            read_markers: HashMap::new(),
            emoji_picker: None,
            message_context: None,
            member_context: None,
            uploading: false,
            upload_cfg: config::UploadConfig::default(),
            file_hover: false,
            search: None,
        };

        match config::load() {
            LoadResult::Loaded(cfg) => {
                build_app_from_cfg(base, cfg, None, now)
            }
            LoadResult::Migrated { cfg, backup } => {
                let note = format!(
                    "config migrated to multi-network format; backup at {}",
                    backup.display()
                );
                build_app_from_cfg(base, cfg, Some(note), now)
            }
            LoadResult::WroteTemplate(path) => App {
                channels: vec![status_channel(
                    NetworkId(0),
                    "",
                    vec![
                        system_line("no config found", now),
                        system_line(&format!("wrote template at {}", path.display()), now),
                        system_line("edit it with your server + nick, then restart.", now),
                    ],
                )],
                fallback_status: ConnStatus::TemplateWritten,
                cfg_path: Some(path),
                ..base
            },
            LoadResult::Error(e) => App {
                channels: vec![status_channel(
                    NetworkId(0),
                    "",
                    vec![
                        system_line("config error:", now),
                        system_line(&e, now),
                    ],
                )],
                fallback_status: ConnStatus::Error,
                last_error: Some(e),
                ..base
            },
        }
    }
}

fn build_app_from_cfg(
    base: App,
    cfg: AppConfig,
    migration_note: Option<String>,
    now: Instant,
) -> App {
    let theme_name = cfg
        .theme
        .clone()
        .filter(|n| themes::by_name(n).is_some())
        .unwrap_or_else(|| "soft-dark".into());
    theme::set(themes::by_name(&theme_name).unwrap_or(themes::SOFT_DARK));

    if cfg.networks.is_empty() {
        return App {
            channels: vec![status_channel(
                NetworkId(0),
                "",
                vec![
                    system_line("no networks defined in config", now),
                    system_line("add a [[network]] block and restart.", now),
                ],
            )],
            fallback_status: ConnStatus::NotConfigured,
            theme_name,
            ..base
        };
    }

    let mut networks: Vec<NetworkState> = Vec::with_capacity(cfg.networks.len());
    let mut channels: Vec<Channel> = Vec::new();
    for (i, ncfg) in cfg.networks.iter().enumerate() {
        let id = NetworkId(i as u32);
        let intro = if ncfg.autoconnect {
            format!("connecting to {}:{}...", ncfg.server, ncfg.port)
        } else {
            format!("autoconnect disabled — /connect {} to dial", ncfg.name)
        };
        let mut intro_msgs = vec![system_line(&intro, now)];
        if let Some(note) = migration_note.as_ref() {
            if i == 0 {
                intro_msgs.insert(0, system_line(note, now));
            }
        }
        channels.push(status_channel(id, "", intro_msgs));
        let last_selected = Some(channels.len() - 1);
        networks.push(NetworkState {
            id,
            cfg: ncfg.clone(),
            status: if ncfg.autoconnect { ConnStatus::Connecting } else { ConnStatus::NotConfigured },
            outgoing: None,
            last_error: None,
            autoconnect_enabled: ncfg.autoconnect,
            last_selected,
            caps_acked: HashSet::new(),
            away_nicks: HashSet::new(),
            accounts: HashMap::new(),
            isupport: irc_worker::ISupport::default(),
        });
    }

    let active = networks.first().map(|n| n.id);
    let selected = networks
        .first()
        .and_then(|n| n.last_selected)
        .unwrap_or(0);

    let highlight_keywords = cfg
        .highlight_keywords
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let ignored_nicks = cfg
        .ignored_nicks
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    App {
        networks,
        active,
        channels,
        selected,
        theme_name,
        highlight_keywords,
        ignored_nicks,
        upload_cfg: cfg.upload.clone(),
        ..base
    }
}

fn status_channel(network_id: NetworkId, topic: &str, messages: Vec<ChatMessage>) -> Channel {
    Channel {
        network_id,
        name: "&status".into(),
        topic: if topic.is_empty() { None } else { Some(topic.into()) },
        members: Vec::new(),
        member_meta: HashMap::new(),
        messages,
        dimm: false,
        hide_joinpart: false,
        hover_anim: new_row_anim(),
        select_anim: new_row_anim(),
        fade_baseline: Instant::now(),
        has_unread: false,
        has_mention: false,
        chathistory_requested: false,
        read_marker_idx: None,
    }
}

// Tokenize an IRC line into (command, args). A leading `:` on a token
// marks the trailing parameter (rest of line is one arg).
fn parse_raw_line(line: &str) -> (String, Vec<String>) {
    let mut parts: Vec<String> = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        if let Some(trailing) = rest.strip_prefix(':') {
            parts.push(trailing.to_string());
            break;
        }
        match rest.find(char::is_whitespace) {
            Some(idx) => {
                parts.push(rest[..idx].to_string());
                rest = rest[idx..].trim_start();
            }
            None => {
                parts.push(rest.to_string());
                break;
            }
        }
    }
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    let cmd = parts.remove(0).to_uppercase();
    (cmd, parts)
}

// Render a horizontal strip of reaction badges below a message:
// each badge shows `<emoji> <count>` with a tinted background.
fn emoji_cell(entry: &'static emoji::Entry) -> Element<'static, Message> {
    mouse_area(
        button(
            container(text(entry.ch).size(sz(18.0)))
                .width(Length::Fixed(EMOJI_CELL as f32))
                .height(Length::Fixed(EMOJI_CELL as f32))
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center),
        )
        .on_press(Message::EmojiInsert(entry.ch))
        .padding(0)
        .style(|_theme, status| {
            let bg = match status {
                button::Status::Hovered => tok::bg_hover(),
                button::Status::Pressed => tok::bg_2(),
                _ => Color::TRANSPARENT,
            };
            button::Style {
                background: Some(Background::Color(bg)),
                text_color: tok::text(),
                border: Border { radius: 6.0.into(), ..Default::default() },
                shadow: Shadow::default(),
                ..Default::default()
            }
        }),
    )
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

fn reactions_row<'a>(
    reactions: &'a HashMap<String, HashSet<String>>,
) -> Element<'a, Message> {
    let mut entries: Vec<(&String, &HashSet<String>)> = reactions.iter().collect();
    entries.sort_by_key(|(emoji, _)| emoji.as_str());
    let mut row_el = row![].spacing(tok::S2);
    for (emoji, reactors) in entries {
        let badge = container(
            text(format!("{emoji} {}", reactors.len()))
                .size(sz(11.0))
                .color(tok::text_mid()),
        )
        .padding(pad(1.0, 6.0, 1.0, 6.0))
        .style(|_| container::Style {
            background: Some(Background::Color(tok::bg_hover())),
            border: Border {
                radius: 8.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });
        row_el = row_el.push(badge);
    }
    container(row_el)
        .padding(pad(0.0, 0.0, tok::S1 as f32, 64.0))
        .into()
}

fn member_action_button<'a>(label: String, msg: Message) -> Element<'a, Message> {
    button(
        text(label)
            .size(sz(11.0))
            .color(tok::text_mid())
            .font(regular()),
    )
    .width(Fill)
    .padding(pad(3.0, 8.0, 3.0, 8.0))
    .on_press(msg)
    .style(|_, status| ghost_button_style(status))
    .into()
}

fn message_action_bar<'a>(ctx: &'a MessageContextState) -> Element<'a, Message> {
    let make_btn = |label: &'static str, msg: Message| -> Element<'a, Message> {
        button(
            text(label)
                .size(sz(11.0))
                .color(tok::text_mid())
                .font(regular()),
        )
        .padding(pad(2.0, 8.0, 2.0, 8.0))
        .on_press(msg)
        .style(|_, status| ghost_button_style(status))
        .into()
    };

    let _ = ctx;
    let mut items: Vec<Element<Message>> = Vec::new();
    items.push(make_btn("☺ React", Message::MessageContextStartReact));
    // Always offer Delete: server enforces ownership/op via FAIL REDACT.
    // Gating on local nick comparison was unreliable across chathistory
    // replay and bouncer-routed echoes.
    items.push(make_btn("✕ Delete", Message::MessageContextDelete));
    items.push(make_btn("Dismiss", Message::MessageContextClose));

    container(row(items).spacing(tok::S2))
        .padding(pad(2.0, 0.0, tok::S1 as f32, 64.0))
        .into()
}

fn system_line(body: &str, now: Instant) -> ChatMessage {
    ChatMessage {
        nick: "*".into(),
        body: body.into(),
        time: now_hhmm(),
        day: "today".into(),
        inserted_at: now,
        mono_secs: 0,
        kind: MsgKind::System,
        msgid: None, reactions: HashMap::new(),
    }
}

fn joinpart_line(body: &str, now: Instant) -> ChatMessage {
    ChatMessage {
        nick: "*".into(),
        body: body.into(),
        time: now_hhmm(),
        day: "today".into(),
        inserted_at: now,
        mono_secs: now.elapsed().as_secs(),
        kind: MsgKind::JoinPart,
        msgid: None, reactions: HashMap::new(),
    }
}

fn chat_line_from_meta(
    nick: String,
    body: String,
    kind: MsgKind,
    meta: &irc_worker::MsgMeta,
    now: Instant,
) -> ChatMessage {
    // Backlog messages (chathistory) skip the fade-in by using an
    // inserted_at well past the fade window — otherwise opening a
    // bouncer-backed channel would burst-animate dozens of lines.
    let is_backlog = meta.batch_kind.as_deref() == Some("chathistory");
    let inserted_at = if is_backlog {
        now.checked_sub(std::time::Duration::from_millis(FADE_MS as u64 * 4))
            .unwrap_or(now)
    } else {
        now
    };
    ChatMessage {
        nick,
        body,
        time: meta.server_time_hhmm.clone().unwrap_or_else(now_hhmm),
        day: "today".into(),
        inserted_at,
        mono_secs: now.elapsed().as_secs(),
        kind,
        msgid: meta.msgid.clone(), reactions: HashMap::new(),
    }
}

fn irc_sub_for_network(
    keyed: &(NetworkId, NetworkConfig),
) -> Pin<Box<dyn Stream<Item = Message> + Send>> {
    use futures::StreamExt;
    let id = keyed.0;
    Box::pin(irc_worker::subscribe(&keyed.1).map(move |ev| Message::Irc(id, ev)))
}

mod chatlog {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;

    fn safe(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '#' | '@' | '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn path(server: &str, channel: &str) -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "murmur").map(|p| {
            p.config_dir()
                .join("logs")
                .join(safe(server))
                .join(format!("{}.log", safe(channel)))
        })
    }

    pub fn append(server: &str, channel: &str, line: &str) {
        if channel.is_empty() || channel.starts_with('&') {
            return;
        }
        let Some(p) = path(server, channel) else {
            return;
        };
        if let Some(parent) = p.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let cleaned = line.replace(['\n', '\r'], " ");
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&p) {
            let _ = writeln!(f, "{cleaned}");
        }
    }

    pub fn iso_now() -> String {
        iso_offset_secs(0)
    }

    /// ISO 8601 timestamp for `days_ago * 86400` seconds before now.
    pub fn iso_minus_days(days_ago: i64) -> String {
        iso_offset_secs(-days_ago * 86_400)
    }

    fn iso_offset_secs(offset: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
            .saturating_add(offset);
        let secs = (now.rem_euclid(60)) as u32;
        let mins = ((now.div_euclid(60)).rem_euclid(60)) as u32;
        let hrs = ((now.div_euclid(3600)).rem_euclid(24)) as u32;
        let days = now.div_euclid(86400);
        let (y, m, d) = days_to_ymd(days);
        format!("{y:04}-{m:02}-{d:02}T{hrs:02}:{mins:02}:{secs:02}Z")
    }

    // Hinnant's civil_from_days — gives the right UTC date for any
    // unix-epoch day count, with no calendar libs.
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
}

fn now_hhmm() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64
        + local_offset_secs();
    let h = s.rem_euclid(86400) / 3600;
    let m = s.rem_euclid(3600) / 60;
    format!("{h:02}:{m:02}")
}

impl App {
    fn idx_of_net(&self, id: NetworkId) -> Option<usize> {
        self.networks.iter().position(|n| n.id == id)
    }

    fn net(&self, id: NetworkId) -> Option<&NetworkState> {
        self.idx_of_net(id).map(|i| &self.networks[i])
    }

    fn net_mut(&mut self, id: NetworkId) -> Option<&mut NetworkState> {
        self.idx_of_net(id).map(move |i| &mut self.networks[i])
    }

    fn active_net(&self) -> Option<&NetworkState> {
        self.active.and_then(|id| self.net(id))
    }

    fn active_net_mut(&mut self) -> Option<&mut NetworkState> {
        let id = self.active?;
        self.net_mut(id)
    }

    fn current_status(&self) -> ConnStatus {
        self.active_net().map(|n| n.status).unwrap_or(self.fallback_status)
    }

    fn current_nickname(&self) -> Option<String> {
        self.active_net().map(|n| n.cfg.nickname.clone())
    }

    fn current_network_name_for_log(&self) -> String {
        self.active_net()
            .map(|n| n.cfg.name.clone())
            .unwrap_or_else(|| "unknown".into())
    }

    // Returns the channel index for `name` within the given network,
    // creating an empty channel if none exists.
    fn find_channel_in(&self, network_id: NetworkId, name: &str) -> Option<usize> {
        self.channels
            .iter()
            .position(|c| c.network_id == network_id && c.name == name)
    }

    // Send a `+typing=active` TAGMSG for the current buffer if the server
    // supports `draft/typing` and we haven't sent one in the last 3 seconds
    // (per spec). Suppressed in &status and on empty input.
    fn maybe_send_typing(&mut self, now: Instant) {
        if self.input.trim().is_empty() {
            self.send_typing_done();
            return;
        }
        let Some(ch) = self.channels.get(self.selected) else { return };
        if ch.name.starts_with('&') {
            return;
        }
        let net_id = ch.network_id;
        let target = ch.name.clone();
        let supported = self
            .net(net_id)
            .is_some_and(|n| n.caps_acked.contains("draft/typing"));
        if !supported {
            return;
        }
        let key = (net_id, target.clone());
        let send_now = self
            .typing_sent
            .get(&key)
            .map(|t| now.duration_since(*t).as_secs() >= 3)
            .unwrap_or(true);
        if !send_now {
            return;
        }
        if let Some(tx) = self.net_mut(net_id).and_then(|n| n.outgoing.as_mut()) {
            let _ = tx.try_send(Outgoing::Typing {
                target,
                state: irc_worker::TypingState::Active,
            });
            self.typing_sent.insert(key, now);
        }
    }

    fn send_typing_done(&mut self) {
        let Some(ch) = self.channels.get(self.selected) else { return };
        if ch.name.starts_with('&') {
            return;
        }
        let net_id = ch.network_id;
        let target = ch.name.clone();
        let key = (net_id, target.clone());
        if self.typing_sent.remove(&key).is_none() {
            return;
        }
        let supported = self
            .net(net_id)
            .is_some_and(|n| n.caps_acked.contains("draft/typing"));
        if !supported {
            return;
        }
        if let Some(tx) = self.net_mut(net_id).and_then(|n| n.outgoing.as_mut()) {
            let _ = tx.try_send(Outgoing::Typing {
                target,
                state: irc_worker::TypingState::Done,
            });
        }
    }

    // Return a comma-separated list of nicks typing in the active buffer.
    // Entries older than 6 seconds are considered expired and ignored.
    fn typing_text(&self) -> Option<String> {
        let ch = self.channels.get(self.selected)?;
        if ch.name.starts_with('&') {
            return None;
        }
        let now = Instant::now();
        let mut nicks: Vec<&str> = self
            .typing_observed
            .iter()
            .filter_map(|((net, bucket, nick), seen)| {
                if *net == ch.network_id
                    && bucket == &ch.name
                    && now.duration_since(*seen).as_secs() < 6
                {
                    Some(nick.as_str())
                } else {
                    None
                }
            })
            .collect();
        if nicks.is_empty() {
            return None;
        }
        nicks.sort();
        let body = match nicks.len() {
            1 => format!("{} is typing…", nicks[0]),
            2 => format!("{} and {} are typing…", nicks[0], nicks[1]),
            _ => format!("{} others are typing…", nicks.len()),
        };
        Some(body)
    }

    // Maps an inbound message's `target` to the channel-buffer name we
    // store it under: DM messages live under the *other* nick's name,
    // channel messages stay under the channel.
    fn bucket_for_target(&self, network_id: NetworkId, target: &str, nick: &str) -> String {
        let my_nick = self
            .net(network_id)
            .map(|n| n.cfg.nickname.clone())
            .unwrap_or_default();
        if !my_nick.is_empty() && target == my_nick {
            nick.to_string()
        } else {
            target.to_string()
        }
    }

    fn ensure_channel_in(&mut self, network_id: NetworkId, name: &str) -> usize {
        if let Some(i) = self
            .channels
            .iter()
            .position(|c| c.network_id == network_id && c.name == name)
        {
            return i;
        }
        self.channels.push(Channel {
            network_id,
            name: name.to_string(),
            topic: None,
            messages: Vec::new(),
            members: Vec::new(),
            member_meta: HashMap::new(),
            dimm: false,
            hide_joinpart: false,
            hover_anim: new_row_anim(),
            select_anim: new_row_anim(),
            fade_baseline: Instant::now(),
            has_unread: false,
            has_mention: false,
            chathistory_requested: false,
            read_marker_idx: None,
        });
        self.channels.len() - 1
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let is_tick = matches!(&message, Message::Tick(_));
        let task = self.dispatch(message);
        if !is_tick {
            let now = Instant::now();
            self.now = now;
            self.sync_channel_animations(now);
        }
        task
    }

    fn dispatch(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ChannelSelected(i) => {
                self.set_selected(i);
                Task::none()
            }
            Message::InputChanged(s) => {
                if let Some(ts) = &self.tab_state {
                    if s != ts.expected_input {
                        self.tab_state = None;
                    }
                }
                if let Some(idx) = self.history_cursor {
                    if self.input_history.get(idx).map(|h| h.as_str()) != Some(s.as_str()) {
                        self.history_cursor = None;
                        self.history_draft.clear();
                    }
                }
                self.input = s;
                self.maybe_send_typing(Instant::now());
                Task::none()
            }
            Message::SendMessage => {
                let text = self.input.trim();
                if text.is_empty() {
                    return Task::none();
                }
                let text = text.to_string();
                self.push_history(&text);
                self.input.clear();
                self.history_cursor = None;
                self.history_draft.clear();
                self.send_typing_done();

                if let Some(cmd) = text.strip_prefix('/') {
                    self.handle_command(cmd);
                    return Task::none();
                }

                let target = self.channels[self.selected].name.clone();
                let is_status = target.starts_with('&');

                if !is_status {
                    if let Some(tx) = self.active_net_mut().and_then(|n| n.outgoing.as_mut()) {
                        let _ = tx.try_send(Outgoing::Privmsg {
                            target: target.clone(),
                            text: text.clone(),
                        });
                    }
                    let nick = self.current_nickname().unwrap_or_else(|| "you".into());
                    let now = Instant::now();
                    let fetch = self.schedule_media_fetches(&text);
                    chatlog::append(
                        &self.current_network_name_for_log(),
                        &target,
                        &format!("{}  <{}> {}", chatlog::iso_now(), nick, text),
                    );
                    self.channels[self.selected].messages.push(ChatMessage {
                        nick,
                        body: text,
                        time: now_hhmm(),
                        day: "today".into(),
                        inserted_at: now,
                        mono_secs: now.elapsed().as_secs(),
                        kind: MsgKind::Chat,
                        msgid: None, reactions: HashMap::new(),
                    });
                    self.now = now;
                    return fetch;
                }
                Task::none()
            }
            Message::ToggleSidebar => {
                let v = !self.sidebar_anim.value();
                self.sidebar_anim.go_mut(v, Instant::now());
                Task::none()
            }
            Message::ToggleMembers => {
                let v = !self.members_anim.value();
                self.members_anim.go_mut(v, Instant::now());
                Task::none()
            }
            Message::Tick(now) => {
                self.now = now;
                Task::none()
            }
            Message::Irc(net_id, event) => self.handle_irc(net_id, event),
            Message::NetworkSelected(id) => {
                self.set_active_network(id);
                Task::none()
            }
            Message::HoverNetwork(v) => {
                self.hovered_network = v;
                Task::none()
            }
            Message::OpenUrl(url) => {
                if let Some(nick) = url.strip_prefix("dm:") {
                    let idx = self.ensure_channel(nick);
                    self.set_selected(idx);
                } else {
                    open_url(&url);
                }
                Task::none()
            }
            Message::WindowFocus(focused) => {
                self.window_focused = focused;
                if focused {
                    self.clear_active_unread();
                } else if let Some(ch) = self.channels.get_mut(self.selected) {
                    ch.read_marker_idx = None;
                }
                Task::none()
            }
            Message::MediaFetched(fetched) => {
                self.media_cache.insert(fetched.url, fetched.state);
                Task::none()
            }
            Message::AttachFilePressed => {
                if self.uploading || !self.has_upload_target() {
                    return Task::none();
                }
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    Message::FilePicked,
                )
            }
            Message::FilePicked(None) => Task::none(),
            Message::FilePicked(Some(path)) => self.start_upload(path),
            Message::FileHovered => {
                if !self.uploading && self.has_upload_target() {
                    self.file_hover = true;
                }
                Task::none()
            }
            Message::FilesHoveredLeft => {
                self.file_hover = false;
                Task::none()
            }
            Message::FileDropped(path) => {
                self.file_hover = false;
                if self.uploading {
                    return Task::none();
                }
                self.start_upload(path)
            }
            Message::SearchClose => {
                self.search = None;
                iced::widget::operation::focus(COMPOSE_INPUT_ID)
            }
            Message::SearchQuery(q) => {
                if let Some(s) = self.search.as_mut() {
                    s.query = q;
                }
                Task::none()
            }
            Message::UploadFinished(result) => {
                self.uploading = false;
                match result {
                    Ok(url) => {
                        if self.input.is_empty() {
                            self.input = url;
                        } else {
                            if !self.input.ends_with(' ') {
                                self.input.push(' ');
                            }
                            self.input.push_str(&url);
                        }
                        iced::widget::operation::focus(COMPOSE_INPUT_ID)
                    }
                    Err(e) => {
                        let now = self.now;
                        let i = self.selected;
                        self.channels[i]
                            .messages
                            .push(system_line(&format!("upload failed: {e}"), now));
                        Task::none()
                    }
                }
            }
            Message::Key(ev) => self.handle_key(ev),
            Message::PaletteQuery(q) => {
                self.palette_query = q;
                self.palette_cursor = 0;
                Task::none()
            }
            Message::PaletteActivate => self.palette_activate(),
            Message::PaletteActivateIdx(i) => {
                self.palette_cursor = i;
                self.palette_activate()
            }
            Message::PaletteClose => {
                self.palette_open = false;
                Task::none()
            }
            Message::HoverChannel(v) => {
                self.hovered_channel = v;
                Task::none()
            }
            Message::HoverMember(v) => {
                self.hovered_member = v;
                Task::none()
            }
            Message::StartDmWith(nick) => {
                let idx = self.ensure_channel(&nick);
                self.set_selected(idx);
                Task::none()
            }
            Message::CloseChannel(i) => {
                if i >= self.channels.len() {
                    return Task::none();
                }
                let name = self.channels[i].name.clone();
                if name.starts_with('&') {
                    return Task::none();
                }
                if name.starts_with('#') {
                    let cid = self.channels[i].network_id;
                    if let Some(tx) = self.net_mut(cid).and_then(|n| n.outgoing.as_mut()) {
                        let _ = tx.try_send(Outgoing::Part {
                            channel: name,
                            reason: None,
                        });
                    }
                }
                self.channels.remove(i);
                let new_sel = if self.selected == i {
                    if i > 0 { i - 1 } else { 0 }
                } else if self.selected > i {
                    self.selected - 1
                } else {
                    self.selected
                };
                self.set_selected(new_sel.min(self.channels.len().saturating_sub(1)));
                Task::none()
            }
            Message::SettingsClose => {
                self.settings_open = false;
                Task::none()
            }
            Message::SettingsSelectSection(s) => {
                self.settings_section = s;
                self.settings_save_error = None;
                self.settings_save_info = None;
                Task::none()
            }
            Message::SettingsThemeChanged(name) => {
                self.settings_draft.theme = Some(name.clone());
                if let Some(p) = themes::by_name(&name) {
                    theme::set(p);
                    self.theme_name = name;
                }
                Task::none()
            }
            Message::SettingsFontFamily(s) => {
                let trimmed = s.trim();
                self.settings_draft.font_family =
                    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                Task::none()
            }
            Message::SettingsFontScale(v) => {
                let v = v.clamp(0.5, 3.0);
                self.settings_draft.font_size_scale = Some(v);
                Task::none()
            }
            Message::SettingsKwInput(s) => {
                self.settings_kw_input = s;
                Task::none()
            }
            Message::SettingsKwAdd => {
                let kw = self.settings_kw_input.trim().to_string();
                if !kw.is_empty()
                    && !self
                        .settings_draft
                        .highlight_keywords
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(&kw))
                {
                    self.settings_draft.highlight_keywords.push(kw);
                }
                self.settings_kw_input.clear();
                Task::none()
            }
            Message::SettingsKwRemove(i) => {
                if i < self.settings_draft.highlight_keywords.len() {
                    self.settings_draft.highlight_keywords.remove(i);
                }
                Task::none()
            }
            Message::SettingsNetSelect(i) => {
                if i < self.settings_draft.networks.len() {
                    self.settings_net_idx = i;
                    self.settings_net_channel_input.clear();
                }
                Task::none()
            }
            Message::SettingsNetAdd => {
                self.settings_draft.networks.push(NetworkConfig {
                    name: format!("network-{}", self.settings_draft.networks.len() + 1),
                    nickname: String::new(),
                    username: None,
                    realname: None,
                    server: String::new(),
                    port: 6697,
                    use_tls: true,
                    nick_password: None,
                    sasl_username: None,
                    sasl_password: None,
                    client_cert_path: None,
                    client_cert_pass: None,
                    channels: Vec::new(),
                    autoconnect: true,
                });
                self.settings_net_idx = self.settings_draft.networks.len() - 1;
                Task::none()
            }
            Message::SettingsNetRemove(i) => {
                if i < self.settings_draft.networks.len() {
                    self.settings_draft.networks.remove(i);
                    if self.settings_draft.networks.is_empty() {
                        self.settings_net_idx = 0;
                    } else if self.settings_net_idx >= self.settings_draft.networks.len() {
                        self.settings_net_idx = self.settings_draft.networks.len() - 1;
                    }
                }
                Task::none()
            }
            Message::SettingsNetField(field, value) => {
                if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx) {
                    let optstr = |v: String| if v.is_empty() { None } else { Some(v) };
                    match field {
                        NetField::Name => n.name = value,
                        NetField::Nickname => n.nickname = value,
                        NetField::Username => n.username = optstr(value),
                        NetField::Realname => n.realname = optstr(value),
                        NetField::Server => n.server = value,
                        NetField::Port => {
                            if value.is_empty() {
                                n.port = 6697;
                            } else if let Ok(p) = value.parse::<u16>() {
                                n.port = p;
                            }
                        }
                        NetField::NickPassword => n.nick_password = Some(value),
                        NetField::SaslUsername => n.sasl_username = optstr(value),
                        NetField::SaslPassword => n.sasl_password = Some(value),
                        NetField::ClientCertPath => n.client_cert_path = Some(value),
                        NetField::ClientCertPass => n.client_cert_pass = Some(value),
                    }
                }
                Task::none()
            }
            Message::SettingsNetTls(v) => {
                if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx) {
                    n.use_tls = v;
                }
                Task::none()
            }
            Message::SettingsNetAutoconnect(v) => {
                if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx) {
                    n.autoconnect = v;
                }
                Task::none()
            }
            Message::SettingsNetAuthMode(mode) => {
                if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx) {
                    // The four modes are mutually exclusive in our auth_mode()
                    // resolver; clear everything, then enable the chosen one
                    // by setting its field to an empty placeholder so the
                    // input row appears.
                    n.nick_password = None;
                    n.sasl_username = None;
                    n.sasl_password = None;
                    n.client_cert_path = None;
                    n.client_cert_pass = None;
                    match mode {
                        SettingsAuthMode::None => {}
                        SettingsAuthMode::NickServ => {
                            n.nick_password = Some(String::new());
                        }
                        SettingsAuthMode::SaslPlain => {
                            n.sasl_password = Some(String::new());
                        }
                        SettingsAuthMode::SaslExternal => {
                            n.client_cert_path = Some(String::new());
                            n.client_cert_pass = Some(String::new());
                        }
                    }
                }
                Task::none()
            }
            Message::SettingsNetChannelInput(s) => {
                self.settings_net_channel_input = s;
                Task::none()
            }
            Message::SettingsNetChannelAdd => {
                let raw = self.settings_net_channel_input.trim();
                if !raw.is_empty() {
                    let ch = if raw.starts_with('#') || raw.starts_with('&') {
                        raw.to_string()
                    } else {
                        format!("#{raw}")
                    };
                    if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx)
                    {
                        if !n.channels.iter().any(|c| c.eq_ignore_ascii_case(&ch)) {
                            n.channels.push(ch);
                        }
                    }
                }
                self.settings_net_channel_input.clear();
                Task::none()
            }
            Message::SettingsNetChannelRemove(i) => {
                if let Some(n) = self.settings_draft.networks.get_mut(self.settings_net_idx) {
                    if i < n.channels.len() {
                        n.channels.remove(i);
                    }
                }
                Task::none()
            }
            Message::SettingsSave => {
                self.save_settings();
                Task::none()
            }
            Message::SettingsUploadUseCustom(b) => {
                self.settings_draft.upload.use_custom = b;
                if b && self.settings_draft.upload.custom.is_none() {
                    self.settings_draft.upload.custom = Some(config::CustomUploader::default());
                }
                Task::none()
            }
            Message::SettingsUploadField(field, val) => {
                let c = self
                    .settings_draft
                    .upload
                    .custom
                    .get_or_insert_with(config::CustomUploader::default);
                match field {
                    UploadField::Url => c.url = val,
                    UploadField::Token => {
                        c.token = if val.is_empty() { None } else { Some(val) }
                    }
                    UploadField::Field => c.field = val,
                    UploadField::ResponseKey => c.response_key = val,
                }
                Task::none()
            }
            Message::SettingsUploadKind(kind) => {
                let c = self
                    .settings_draft
                    .upload
                    .custom
                    .get_or_insert_with(config::CustomUploader::default);
                c.response_kind = kind;
                Task::none()
            }
            Message::EmojiPickerToggle => {
                if self.emoji_picker.is_some() {
                    self.emoji_picker = None;
                    iced::widget::operation::focus(COMPOSE_INPUT_ID)
                } else {
                    self.emoji_picker = Some(EmojiPickerState::default());
                    iced::widget::operation::focus(EMOJI_PICKER_INPUT_ID)
                }
            }
            Message::EmojiPickerClose => {
                self.emoji_picker = None;
                iced::widget::operation::focus(COMPOSE_INPUT_ID)
            }
            Message::EmojiPickerQuery(q) => {
                if let Some(p) = self.emoji_picker.as_mut() {
                    p.query = q;
                }
                Task::none()
            }
            Message::EmojiPickerCategory(cat) => {
                if let Some(p) = self.emoji_picker.as_mut() {
                    p.category = if p.category == Some(cat) { None } else { Some(cat) };
                }
                Task::none()
            }
            Message::EmojiInsert(ch) => {
                let target = self
                    .emoji_picker
                    .as_ref()
                    .and_then(|p| p.react_target.clone());
                self.emoji_picker = None;
                if let Some(rt) = target {
                    let now = Instant::now();
                    if let Some(channel) = self.channels.get(rt.channel_idx) {
                        let target_name = channel.name.clone();
                        self.send_out(
                            Outgoing::React {
                                target: target_name,
                                msgid: rt.msgid,
                                emoji: ch.to_string(),
                            },
                            now,
                        );
                    }
                } else {
                    self.input.push_str(ch);
                }
                iced::widget::operation::focus(COMPOSE_INPUT_ID)
            }
            Message::MessageContextOpen { channel_idx, msgid } => {
                self.message_context = Some(MessageContextState {
                    channel_idx,
                    msgid,
                });
                Task::none()
            }
            Message::MessageContextClose => {
                self.message_context = None;
                Task::none()
            }
            Message::MessageContextDelete => {
                if let Some(ctx) = self.message_context.take() {
                    let now = Instant::now();
                    if let Some(channel) = self.channels.get(ctx.channel_idx) {
                        let target = channel.name.clone();
                        self.send_out(
                            Outgoing::Redact {
                                target,
                                msgid: ctx.msgid,
                                reason: None,
                            },
                            now,
                        );
                    }
                }
                Task::none()
            }
            Message::MessageContextStartReact => {
                if let Some(ctx) = self.message_context.take() {
                    self.emoji_picker = Some(EmojiPickerState {
                        react_target: Some(ReactTarget {
                            channel_idx: ctx.channel_idx,
                            msgid: ctx.msgid,
                        }),
                        ..EmojiPickerState::default()
                    });
                    iced::widget::operation::focus(EMOJI_PICKER_INPUT_ID)
                } else {
                    Task::none()
                }
            }
            Message::MemberContextOpen { nick } => {
                self.member_context = Some(MemberContextState {
                    channel_idx: self.selected,
                    nick,
                });
                Task::none()
            }
            Message::MemberContextClose => {
                self.member_context = None;
                Task::none()
            }
            Message::MemberContextDm => {
                if let Some(ctx) = self.member_context.take() {
                    let idx = self.ensure_channel(&ctx.nick);
                    self.set_selected(idx);
                }
                Task::none()
            }
            Message::MemberContextWhois => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    self.cmd_whois(&ctx.nick, now);
                }
                Task::none()
            }
            Message::MemberContextIgnoreToggle => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    if self.is_ignored(&ctx.nick) {
                        self.cmd_unignore(&ctx.nick, now);
                    } else {
                        self.cmd_ignore(&ctx.nick, now);
                    }
                }
                Task::none()
            }
            Message::MemberContextOpToggle => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    let sign = if matches!(
                        self.member_prefix(&ctx.nick),
                        Some('~') | Some('&') | Some('@')
                    ) { "-" } else { "+" };
                    self.cmd_channel_priv(sign, "o", &ctx.nick, now);
                }
                Task::none()
            }
            Message::MemberContextVoiceToggle => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    let sign = if matches!(self.member_prefix(&ctx.nick), Some('+')) {
                        "-"
                    } else {
                        "+"
                    };
                    self.cmd_channel_priv(sign, "v", &ctx.nick, now);
                }
                Task::none()
            }
            Message::MemberContextKick => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    self.cmd_kick(&ctx.nick, now);
                }
                Task::none()
            }
            Message::MemberContextBan => {
                if let Some(ctx) = self.member_context.take() {
                    let now = Instant::now();
                    self.cmd_ban(true, &ctx.nick, now);
                }
                Task::none()
            }
        }
    }

    fn open_settings(&mut self) {
        self.settings_draft = self.build_current_config();
        self.settings_open = true;
        self.settings_section = SettingsSection::Appearance;
        self.settings_save_error = None;
        self.settings_save_info = None;
        self.settings_kw_input.clear();
        self.settings_net_channel_input.clear();
        if self.settings_draft.networks.is_empty() {
            self.settings_net_idx = 0;
        } else if self.settings_net_idx >= self.settings_draft.networks.len() {
            self.settings_net_idx = self.settings_draft.networks.len() - 1;
        }
    }

    fn build_current_config(&self) -> AppConfig {
        let mut ignored: Vec<String> = self.ignored_nicks.iter().cloned().collect();
        ignored.sort();
        AppConfig {
            networks: self.networks.iter().map(|n| n.cfg.clone()).collect(),
            theme: Some(self.theme_name.clone()),
            font_family: USER_FONT.get().map(|s| s.to_string()),
            font_size_scale: FONT_SCALE.get().copied(),
            highlight_keywords: self.highlight_keywords.clone(),
            ignored_nicks: ignored,
            upload: self.upload_cfg.clone(),
        }
    }

    fn save_settings(&mut self) {
        let Some(path) = config::config_path() else {
            self.settings_save_error =
                Some("could not resolve config directory".into());
            return;
        };
        // The settings panel doesn't expose ignored_nicks — merge the
        // live set so /ignore changes made while settings was open are
        // not silently wiped on save.
        let mut ignored: Vec<String> = self.ignored_nicks.iter().cloned().collect();
        ignored.sort();
        self.settings_draft.ignored_nicks = ignored;
        let toml_text = match toml::to_string_pretty(&self.settings_draft) {
            Ok(s) => s,
            Err(e) => {
                self.settings_save_error = Some(format!("serialize: {e}"));
                return;
            }
        };
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.settings_save_error = Some(format!("create dir: {e}"));
                return;
            }
        }
        if let Err(e) = std::fs::write(&path, &toml_text) {
            self.settings_save_error = Some(format!("write: {e}"));
            return;
        }
        // Live-apply settings that don't require a restart.
        if let Some(name) = self.settings_draft.theme.clone() {
            if let Some(p) = themes::by_name(&name) {
                theme::set(p);
                self.theme_name = name;
            }
        }
        self.highlight_keywords = self.settings_draft.highlight_keywords.clone();
        self.upload_cfg = self.settings_draft.upload.clone();
        self.settings_save_error = None;
        self.settings_save_info = Some(
            "saved · font, font scale and network changes need a restart"
                .into(),
        );
    }

    fn handle_key(&mut self, ev: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed { key, modifiers, .. } = ev else {
            return Task::none();
        };
        let is_cmd_k = matches!(&key, keyboard::Key::Character(c) if c.eq_ignore_ascii_case("k"))
            && (modifiers.command() || modifiers.control());

        if is_cmd_k {
            if self.palette_open {
                self.palette_open = false;
                return Task::none();
            }
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_cursor = 0;
            return iced::widget::operation::focus(PALETTE_INPUT_ID);
        }

        // Cmd/Ctrl + , → open settings (closes if already open)
        let is_cmd_comma = matches!(&key, keyboard::Key::Character(c) if c.as_str() == ",")
            && (modifiers.command() || modifiers.control());
        if is_cmd_comma {
            if self.settings_open {
                self.settings_open = false;
            } else {
                self.open_settings();
            }
            return Task::none();
        }

        // Cmd/Ctrl + W → close current buffer
        let is_cmd_w = matches!(&key, keyboard::Key::Character(c) if c.as_str().eq_ignore_ascii_case("w"))
            && (modifiers.command() || modifiers.control())
            && !self.palette_open
            && !self.settings_open;
        if is_cmd_w {
            self.cmd_close(Instant::now());
            return Task::none();
        }

        // Cmd/Ctrl + F → toggle in-buffer search overlay.
        let is_cmd_f = matches!(&key, keyboard::Key::Character(c) if c.as_str().eq_ignore_ascii_case("f"))
            && (modifiers.command() || modifiers.control());
        if is_cmd_f {
            if self.search.is_some() {
                self.search = None;
                return iced::widget::operation::focus(COMPOSE_INPUT_ID);
            }
            self.search = Some(SearchState::default());
            return iced::widget::operation::focus(SEARCH_INPUT_ID);
        }

        // Esc closes the search overlay when active.
        if self.search.is_some()
            && matches!(&key, keyboard::Key::Named(keyboard::key::Named::Escape))
        {
            self.search = None;
            return iced::widget::operation::focus(COMPOSE_INPUT_ID);
        }

        // Esc closes settings when it's the top-most overlay.
        if self.settings_open
            && matches!(&key, keyboard::Key::Named(keyboard::key::Named::Escape))
        {
            self.settings_open = false;
            return Task::none();
        }

        // Esc closes the emoji picker when it's the active overlay.
        if self.emoji_picker.is_some()
            && matches!(&key, keyboard::Key::Named(keyboard::key::Named::Escape))
        {
            self.emoji_picker = None;
            return iced::widget::operation::focus(COMPOSE_INPUT_ID);
        }

        // Esc closes the per-message action bar.
        if self.message_context.is_some()
            && matches!(&key, keyboard::Key::Named(keyboard::key::Named::Escape))
        {
            self.message_context = None;
            return iced::widget::operation::focus(COMPOSE_INPUT_ID);
        }

        // Esc closes the per-member action bar.
        if self.member_context.is_some()
            && matches!(&key, keyboard::Key::Named(keyboard::key::Named::Escape))
        {
            self.member_context = None;
            return iced::widget::operation::focus(COMPOSE_INPUT_ID);
        }

        if matches!(&key, keyboard::Key::Named(keyboard::key::Named::Tab))
            && !self.palette_open
        {
            if self.try_tab_complete() {
                return iced::widget::operation::move_cursor_to_end(COMPOSE_INPUT_ID);
            }
            return Task::none();
        }

        if !self.palette_open {
            match key {
                keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                    if self.history_up() {
                        return iced::widget::operation::move_cursor_to_end(COMPOSE_INPUT_ID);
                    }
                }
                keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                    if self.history_down() {
                        return iced::widget::operation::move_cursor_to_end(COMPOSE_INPUT_ID);
                    }
                }
                _ => {}
            }
            return Task::none();
        }
        match key {
            keyboard::Key::Named(keyboard::key::Named::Escape) => {
                self.palette_open = false;
            }
            keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                self.palette_cursor = self.palette_cursor.saturating_sub(1);
            }
            keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                let len = self.filtered_palette_items().len();
                if len > 0 {
                    self.palette_cursor = (self.palette_cursor + 1).min(len - 1);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn palette_activate(&mut self) -> Task<Message> {
        if !self.palette_open {
            return Task::none();
        }
        let query = self.palette_query.trim().to_string();
        if let Some(cmd) = query.strip_prefix('/') {
            self.palette_open = false;
            self.palette_query.clear();
            self.palette_cursor = 0;
            if !cmd.is_empty() {
                self.handle_command(cmd);
            }
            return Task::none();
        }
        let items = self.filtered_palette_items();
        let Some(item) = items.get(self.palette_cursor).cloned() else {
            return Task::none();
        };
        match item {
            PaletteItem::Channel(i) => {
                self.set_selected(i);
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_cursor = 0;
                Task::none()
            }
            PaletteItem::Command { name, needs_args, .. } => {
                if needs_args {
                    self.palette_query = format!("{name} ");
                    self.palette_cursor = 0;
                    iced::widget::operation::focus(PALETTE_INPUT_ID)
                } else {
                    self.palette_open = false;
                    self.palette_query.clear();
                    self.palette_cursor = 0;
                    self.handle_command(name.trim_start_matches('/'));
                    Task::none()
                }
            }
        }
    }

    fn schedule_media_fetches(&mut self, body: &str) -> Task<Message> {
        let urls: Vec<String> = extract_urls(body)
            .into_iter()
            .filter(|u| !self.media_cache.contains_key(u))
            .collect();
        if urls.is_empty() {
            return Task::none();
        }
        let mut tasks = Vec::new();
        for url in urls {
            self.media_cache.insert(url.clone(), MediaState::Loading);
            tasks.push(Task::perform(fetch_media(url), Message::MediaFetched));
        }
        Task::batch(tasks)
    }

    // True when there's a usable upload destination for the attach button:
    // either a configured custom uploader, or a FILEHOST endpoint advertised
    // by the current channel's network.
    fn has_upload_target(&self) -> bool {
        if self.upload_cfg.use_custom {
            self.upload_cfg
                .custom
                .as_ref()
                .is_some_and(|c| !c.url.trim().is_empty())
        } else {
            self.channels
                .get(self.selected)
                .and_then(|ch| self.net(ch.network_id))
                .is_some_and(|n| n.isupport.filehost.is_some())
        }
    }

    // Build the concrete upload job for the active backend. Ok(None) means
    // "no target configured"; Err(msg) means a target exists but can't be used
    // (e.g. an insecure endpoint over TLS) and msg should be shown to the user.
    fn resolve_upload_job(&self) -> Result<Option<UploadJob>, String> {
        if self.upload_cfg.use_custom {
            let Some(c) = self.upload_cfg.custom.as_ref() else {
                return Ok(None);
            };
            if c.url.trim().is_empty() {
                return Ok(None);
            }
            return Ok(Some(UploadJob::Custom {
                url: c.url.trim().to_string(),
                token: c.token.clone().filter(|t| !t.is_empty()),
                field: c.field.clone(),
                response_kind: c.response_kind.clone(),
                response_key: c.response_key.clone(),
            }));
        }
        let Some(ch) = self.channels.get(self.selected) else {
            return Ok(None);
        };
        let Some(net) = self.net(ch.network_id) else {
            return Ok(None);
        };
        let Some(endpoint) = net.isupport.filehost.clone() else {
            return Ok(None);
        };
        if net.cfg.use_tls && endpoint.starts_with("http://") {
            return Err(
                "upload refused: FILEHOST endpoint is plaintext http:// over a TLS connection"
                    .into(),
            );
        }
        let auth = {
            let pass = net.cfg.sasl_password.clone().unwrap_or_default();
            if pass.is_empty() {
                None
            } else {
                Some((net.cfg.sasl_user().to_string(), pass))
            }
        };
        Ok(Some(UploadJob::Filehost { endpoint, auth }))
    }

    /// ASCII-lowercased current search query, if the search overlay is open
    /// and the query is non-empty after trimming. Used to drive in-message
    /// highlighting.
    fn search_query_lower(&self) -> Option<String> {
        let s = self.search.as_ref()?;
        let q = s.query.trim();
        if q.is_empty() {
            return None;
        }
        Some(q.to_ascii_lowercase())
    }

    fn search_match_count(&self) -> usize {
        let Some(q) = self.search_query_lower() else {
            return 0;
        };
        let Some(ch) = self.channels.get(self.selected) else {
            return 0;
        };
        ch.messages
            .iter()
            .map(|m| m.body.to_ascii_lowercase().matches(q.as_str()).count())
            .sum()
    }

    fn start_upload(&mut self, path: std::path::PathBuf) -> Task<Message> {
        match self.resolve_upload_job() {
            Ok(Some(job)) => {
                self.uploading = true;
                Task::perform(run_upload(job, path), Message::UploadFinished)
            }
            Ok(None) => Task::none(),
            Err(msg) => {
                let now = self.now;
                let i = self.selected;
                self.channels[i].messages.push(system_line(&msg, now));
                Task::none()
            }
        }
    }

    fn server_for_log(&self) -> String {
        self.current_network_name_for_log()
    }

    fn push_history(&mut self, text: &str) {
        const MAX_HISTORY: usize = 200;
        if text.is_empty() {
            return;
        }
        if self.input_history.last().map(|s| s.as_str()) == Some(text) {
            return;
        }
        self.input_history.push(text.to_string());
        let len = self.input_history.len();
        if len > MAX_HISTORY {
            self.input_history.drain(0..len - MAX_HISTORY);
        }
    }

    fn history_up(&mut self) -> bool {
        if self.input_history.is_empty() {
            return false;
        }
        let next = match self.history_cursor {
            None => {
                self.history_draft = self.input.clone();
                self.input_history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(next);
        self.input = self.input_history[next].clone();
        self.tab_state = None;
        true
    }

    fn history_down(&mut self) -> bool {
        let cur = match self.history_cursor {
            None => return false,
            Some(i) => i,
        };
        if cur + 1 >= self.input_history.len() {
            self.history_cursor = None;
            self.input = std::mem::take(&mut self.history_draft);
        } else {
            let next = cur + 1;
            self.history_cursor = Some(next);
            self.input = self.input_history[next].clone();
        }
        self.tab_state = None;
        true
    }

    fn try_tab_complete(&mut self) -> bool {
        if let Some(ts) = &mut self.tab_state {
            if self.input == ts.expected_input && !ts.matches.is_empty() {
                ts.idx = (ts.idx + 1) % ts.matches.len();
                let m = ts.matches[ts.idx].clone();
                let new_input = format!("{}{}{}", &self.input[..ts.word_start], m, ts.suffix);
                ts.expected_input = new_input.clone();
                self.input = new_input;
                return true;
            }
        }

        let start = last_word_start(&self.input);
        let raw = &self.input[start..];

        let (matches, suffix) = if raw.starts_with('/') && start == 0 {
            let p = raw.trim_start_matches('/').to_lowercase();
            let ms: Vec<String> = PALETTE_COMMANDS
                .iter()
                .filter_map(|(name, _, _)| {
                    name.trim_start_matches('/')
                        .to_lowercase()
                        .starts_with(&p)
                        .then(|| (*name).to_string())
                })
                .collect();
            (ms, " ")
        } else {
            let stripped = raw.strip_prefix('@').unwrap_or(raw);
            if stripped.is_empty() {
                return false;
            }
            let p = stripped.to_lowercase();
            let ms: Vec<String> = self
                .channels
                .get(self.selected)
                .map(|ch| {
                    ch.members
                        .iter()
                        .filter(|n| n.to_lowercase().starts_with(&p))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (ms, if start == 0 { ": " } else { " " })
        };

        if matches.is_empty() {
            return false;
        }

        let m = matches[0].clone();
        let new_input = format!("{}{}{}", &self.input[..start], m, suffix);
        self.tab_state = Some(TabState {
            word_start: start,
            matches,
            idx: 0,
            suffix,
            expected_input: new_input.clone(),
        });
        self.input = new_input;
        true
    }

    fn filtered_palette_items(&self) -> Vec<PaletteItem> {
        let q = self.palette_query.trim().to_lowercase();
        let mut scored: Vec<(u8, PaletteItem)> = Vec::new();
        let push = |scored: &mut Vec<(u8, PaletteItem)>, label: &str, item: PaletteItem| {
            if q.is_empty() {
                scored.push((1, item));
                return;
            }
            let lower = label.to_lowercase();
            if lower.starts_with(&q) {
                scored.push((3, item));
            } else if lower.contains(&q) {
                scored.push((2, item));
            }
        };
        for (i, ch) in self.channels.iter().enumerate() {
            push(&mut scored, &ch.name, PaletteItem::Channel(i));
        }
        for &(name, hint, needs_args) in PALETTE_COMMANDS {
            push(
                &mut scored,
                name,
                PaletteItem::Command { name, hint, needs_args },
            );
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, it)| it).collect()
    }

    fn handle_command(&mut self, cmd: &str) {
        let now = Instant::now();
        let (name, rest) = match cmd.split_once(char::is_whitespace) {
            Some((n, r)) => (n.to_lowercase(), r.trim_start()),
            None => (cmd.to_lowercase(), ""),
        };
        match name.as_str() {
            "dimm" => self.cmd_dimm(rest, now),
            "join" | "j" => self.cmd_join(rest, now),
            "part" | "leave" => self.cmd_part(rest, now),
            "nick" => self.cmd_nick(rest, now),
            "me" => self.cmd_me(rest, now),
            "msg" => self.cmd_msg(rest, now),
            "query" => self.cmd_query(rest, now),
            "theme" => self.cmd_theme(rest, now),
            "config" => self.cmd_config(rest, now),
            "ctcp" => self.cmd_ctcp(rest, now),
            "ping" => self.cmd_ping(rest, now),
            "hidejoins" | "joins" | "joinpart" => self.cmd_hide_joins(now),
            "logs" => self.cmd_logs(now),
            "server" => self.cmd_server(rest, now),
            "connect" => self.cmd_connect(rest, now),
            "disconnect" => self.cmd_disconnect(rest, now),
            "settings" => { self.open_settings(); }
            "close" | "wc" | "q" => self.cmd_close(now),
            "ignore" => self.cmd_ignore(rest, now),
            "unignore" => self.cmd_unignore(rest, now),
            "ignores" => self.cmd_ignores(now),
            "away" => self.cmd_away(rest, now),
            "back" => self.cmd_back(now),
            "whois" | "wi" => self.cmd_whois(rest, now),
            "topic" => self.cmd_topic(rest, now),
            "clear" => self.cmd_clear(now),
            "raw" | "quote" => self.cmd_raw(rest, now),
            "kick" => self.cmd_kick(rest, now),
            "mode" => self.cmd_mode(rest, now),
            "op" => self.cmd_channel_priv("+", "o", rest, now),
            "deop" => self.cmd_channel_priv("-", "o", rest, now),
            "voice" => self.cmd_channel_priv("+", "v", rest, now),
            "devoice" => self.cmd_channel_priv("-", "v", rest, now),
            "ban" => self.cmd_ban(true, rest, now),
            "unban" => self.cmd_ban(false, rest, now),
            "invite" => self.cmd_invite(rest, now),
            "history" => self.cmd_history(now),
            "delete" | "redact" => self.cmd_delete(rest, now),
            "react" => self.cmd_react(rest, now),
            "setname" => self.cmd_setname(rest, now),
            "msgid" => self.cmd_msgid(rest, now),
            "caps" => self.cmd_caps(now),
            other => {
                self.channels[self.selected]
                    .messages
                    .push(system_line(&format!("unknown command: /{other}"), now));
            }
        }
    }

    fn cmd_dimm(&mut self, rest: &str, now: Instant) {
        match rest.split_whitespace().next() {
            None => {
                let ch = &mut self.channels[self.selected];
                ch.dimm = !ch.dimm;
                let msg = if ch.dimm {
                    "focus mode on — non-mentions dimmed"
                } else {
                    "focus mode off"
                };
                ch.messages.push(system_line(msg, now));
            }
            Some(nick) => {
                let nick = nick.to_string();
                let msg = if self.dimmed_nicks.contains(&nick) {
                    self.dimmed_nicks.remove(&nick);
                    format!("{nick} no longer dimmed")
                } else {
                    self.dimmed_nicks.insert(nick.clone());
                    format!("{nick} dimmed — soft-ignored")
                };
                self.channels[self.selected]
                    .messages
                    .push(system_line(&msg, now));
            }
        }
    }

    fn cmd_join(&mut self, rest: &str, now: Instant) {
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /join #channel", now));
            return;
        }
        let chantypes = self.active_chantypes();
        let starts_with_chantype = target
            .chars()
            .next()
            .map(|c| chantypes.contains(c))
            .unwrap_or(false);
        let channel = if starts_with_chantype {
            target.to_string()
        } else {
            // Fall back to '#' as the default channel prefix.
            format!("#{target}")
        };
        if !self.send_out(Outgoing::Join(channel.clone()), now) {
            return;
        }
        let idx = self.ensure_channel(&channel);
        self.set_selected(idx);
    }

    fn cmd_part(&mut self, rest: &str, now: Instant) {
        let channel = self.channels[self.selected].name.clone();
        if !channel.starts_with('#') && !channel.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("not a channel — nothing to part", now));
            return;
        }
        if channel.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("can't part the status buffer", now));
            return;
        }
        let reason = if rest.is_empty() { None } else { Some(rest.to_string()) };
        self.send_out(Outgoing::Part { channel, reason }, now);
    }

    // Write the current in-memory state back to config.toml. Used by
    // commands that mutate persisted prefs (/ignore, /unignore).
    fn persist_config(&self) -> Result<(), String> {
        let path = config::config_path()
            .ok_or_else(|| "could not resolve config directory".to_string())?;
        let cfg = self.build_current_config();
        let text = toml::to_string_pretty(&cfg)
            .map_err(|e| format!("serialize: {e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("write: {e}"))
    }

    fn cmd_ignore(&mut self, rest: &str, now: Instant) {
        let nick = rest.split_whitespace().next().unwrap_or("");
        if nick.is_empty() {
            let mut listed: Vec<String> = self.ignored_nicks.iter().cloned().collect();
            listed.sort();
            let body = if listed.is_empty() {
                "no nicks ignored".to_string()
            } else {
                format!("ignored ({}): {}", listed.len(), listed.join(", "))
            };
            self.channels[self.selected].messages.push(system_line(&body, now));
            return;
        }
        let key = nick.to_ascii_lowercase();
        if !self.ignored_nicks.insert(key) {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("already ignoring {nick}"), now));
            return;
        }
        let msg = match self.persist_config() {
            Ok(()) => format!("ignoring {nick}"),
            Err(e) => format!("ignoring {nick} (persist failed: {e})"),
        };
        self.channels[self.selected].messages.push(system_line(&msg, now));
    }

    fn cmd_unignore(&mut self, rest: &str, now: Instant) {
        let nick = rest.split_whitespace().next().unwrap_or("");
        if nick.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /unignore <nick>", now));
            return;
        }
        let key = nick.to_ascii_lowercase();
        if !self.ignored_nicks.remove(&key) {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("{nick} was not ignored"), now));
            return;
        }
        let msg = match self.persist_config() {
            Ok(()) => format!("no longer ignoring {nick}"),
            Err(e) => format!("unignored {nick} (persist failed: {e})"),
        };
        self.channels[self.selected].messages.push(system_line(&msg, now));
    }

    fn cmd_ignores(&mut self, now: Instant) {
        let mut listed: Vec<String> = self.ignored_nicks.iter().cloned().collect();
        listed.sort();
        let body = if listed.is_empty() {
            "no nicks ignored".to_string()
        } else {
            format!("ignored ({}): {}", listed.len(), listed.join(", "))
        };
        self.channels[self.selected].messages.push(system_line(&body, now));
    }

    fn cmd_away(&mut self, rest: &str, now: Instant) {
        let reason = rest.trim();
        let msg = if reason.is_empty() {
            Some("away".to_string())
        } else {
            Some(reason.to_string())
        };
        if !self.send_out(Outgoing::Away(msg.clone()), now) {
            return;
        }
        let body = format!("away set: {}", msg.as_deref().unwrap_or("away"));
        self.channels[self.selected].messages.push(system_line(&body, now));
    }

    fn cmd_back(&mut self, now: Instant) {
        if !self.send_out(Outgoing::Away(None), now) {
            return;
        }
        self.channels[self.selected]
            .messages
            .push(system_line("back — away cleared", now));
    }

    fn cmd_setname(&mut self, rest: &str, now: Instant) {
        let new = rest.trim();
        if new.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /setname <new realname>", now));
            return;
        }
        let net_id = self.channels[self.selected].network_id;
        let supports = self
            .net(net_id)
            .is_some_and(|n| n.caps_acked.contains("setname"));
        if !supports {
            self.channels[self.selected].messages.push(system_line(
                "server doesn't advertise the `setname` capability",
                now,
            ));
            return;
        }
        self.send_out(Outgoing::SetName(new.to_string()), now);
    }

    fn cmd_whois(&mut self, rest: &str, now: Instant) {
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /whois <nick>", now));
            return;
        }
        self.send_out(Outgoing::Whois(target.to_string()), now);
    }

    fn cmd_topic(&mut self, rest: &str, now: Instant) {
        let channel = self.channels[self.selected].name.clone();
        if !channel.starts_with('#') && !channel.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("not a channel — /topic is channel-only", now));
            return;
        }
        if channel.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("can't /topic the status buffer", now));
            return;
        }
        if rest.trim().is_empty() {
            let body = match self.channels[self.selected].topic.clone() {
                Some(t) if !t.is_empty() => format!("topic for {channel}: {t}"),
                _ => format!("no topic set for {channel}"),
            };
            self.channels[self.selected].messages.push(system_line(&body, now));
            return;
        }
        let topic = rest.to_string();
        self.send_out(
            Outgoing::Topic { channel, topic: Some(topic) },
            now,
        );
    }

    fn cmd_clear(&mut self, _now: Instant) {
        if self.selected < self.channels.len() {
            self.channels[self.selected].messages.clear();
        }
    }

    fn cmd_raw(&mut self, rest: &str, now: Instant) {
        let line = rest.trim();
        if line.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /raw <IRC line>", now));
            return;
        }
        let (cmd, args) = parse_raw_line(line);
        if cmd.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("invalid raw line", now));
            return;
        }
        if !self.send_out(Outgoing::Raw { cmd: cmd.clone(), args }, now) {
            return;
        }
        self.channels[self.selected]
            .messages
            .push(system_line(&format!("→ {cmd}"), now));
    }

    fn current_channel_for_op(&mut self, cmd: &str, now: Instant) -> Option<String> {
        let name = self.channels[self.selected].name.clone();
        if name.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("/{cmd} only works inside a channel"), now));
            return None;
        }
        if !name.starts_with('#') {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("/{cmd} only works inside a channel"), now));
            return None;
        }
        Some(name)
    }

    fn cmd_kick(&mut self, rest: &str, now: Instant) {
        let Some(channel) = self.current_channel_for_op("kick", now) else { return };
        let (nick, reason) = match rest.split_once(char::is_whitespace) {
            Some((n, r)) => {
                let r = r.trim();
                (n.to_string(), if r.is_empty() { None } else { Some(r.to_string()) })
            }
            None => (rest.trim().to_string(), None),
        };
        if nick.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /kick <nick> [reason]", now));
            return;
        }
        self.send_out(Outgoing::Kick { channel, nick, reason }, now);
    }

    fn cmd_mode(&mut self, rest: &str, now: Instant) {
        let mut parts = rest.split_whitespace();
        let first = parts.next().unwrap_or("");
        if first.is_empty() {
            // No args — show mode of the current channel if we're in one.
            if let Some(channel) = self
                .channels
                .get(self.selected)
                .map(|c| c.name.clone())
                .filter(|n| n.starts_with('#'))
            {
                self.send_out(
                    Outgoing::Mode { target: channel, modes: String::new(), args: vec![] },
                    now,
                );
                return;
            }
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /mode <target> [<modes> [args...]]", now));
            return;
        }
        let target = first.to_string();
        let modes = parts.next().unwrap_or("").to_string();
        let args: Vec<String> = parts.map(str::to_string).collect();
        self.send_out(Outgoing::Mode { target, modes, args }, now);
    }

    // Bulk MODE +o/-o/+v/-v helper. Chunks nicks per the server's MODES=
    // ISUPPORT advertisement (default 3 when missing, matching RFC2812).
    fn cmd_channel_priv(&mut self, sign: &str, flag: &str, rest: &str, now: Instant) {
        let label = format!("{sign}{flag}");
        let Some(channel) = self.current_channel_for_op(&label, now) else { return };
        let nicks: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
        if nicks.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("usage: /{label} <nick> [nick...]"), now));
            return;
        }
        let limit = self
            .active_net()
            .and_then(|n| n.isupport.modes)
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(3);
        for chunk in nicks.chunks(limit) {
            let modes = format!("{sign}{}", flag.repeat(chunk.len()));
            let args: Vec<String> = chunk.to_vec();
            if !self.send_out(
                Outgoing::Mode { target: channel.clone(), modes, args },
                now,
            ) {
                return;
            }
        }
    }

    fn cmd_ban(&mut self, ban: bool, rest: &str, now: Instant) {
        let label = if ban { "ban" } else { "unban" };
        let Some(channel) = self.current_channel_for_op(label, now) else { return };
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("usage: /{label} <nick|mask>"), now));
            return;
        }
        // Bare nick → nick!*@*; preserve full masks as-is.
        let mask = if target.contains('!') || target.contains('@') {
            target.to_string()
        } else {
            format!("{target}!*@*")
        };
        let modes = if ban { "+b" } else { "-b" }.to_string();
        self.send_out(
            Outgoing::Mode { target: channel, modes, args: vec![mask] },
            now,
        );
    }

    fn cmd_invite(&mut self, rest: &str, now: Instant) {
        let mut parts = rest.split_whitespace();
        let nick = parts.next().unwrap_or("").to_string();
        if nick.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /invite <nick> [#channel]", now));
            return;
        }
        let channel = match parts.next() {
            Some(c) => c.to_string(),
            None => {
                let name = self.channels[self.selected].name.clone();
                if !name.starts_with('#') {
                    self.channels[self.selected].messages.push(system_line(
                        "usage: /invite <nick> <#channel> (no channel context here)",
                        now,
                    ));
                    return;
                }
                name
            }
        };
        self.send_out(Outgoing::Invite { nick, channel }, now);
    }

    fn cmd_delete(&mut self, rest: &str, now: Instant) {
        let Some(ch) = self.channels.get(self.selected) else { return };
        if ch.name.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("can't /delete in the status buffer", now));
            return;
        }
        let net_id = ch.network_id;
        let target = ch.name.clone();
        let my_nick = self
            .net(net_id)
            .map(|n| n.cfg.nickname.clone())
            .unwrap_or_default();
        // Resolve msgid: explicit arg, or last own message with a msgid.
        let arg = rest.split_whitespace().next().unwrap_or("");
        let msgid = if !arg.is_empty() {
            arg.to_string()
        } else {
            match self.channels[self.selected]
                .messages
                .iter()
                .rev()
                .find(|m| m.nick == my_nick && m.msgid.is_some())
                .and_then(|m| m.msgid.clone())
            {
                Some(id) => id,
                None => {
                    self.channels[self.selected].messages.push(system_line(
                        "no recent message of yours has a msgid (need echo-message or a passed argument)",
                        now,
                    ));
                    return;
                }
            }
        };
        if !self.send_out(
            Outgoing::Redact {
                target,
                msgid,
                reason: None,
            },
            now,
        ) {
            return;
        }
    }

    fn cmd_react(&mut self, rest: &str, now: Instant) {
        let Some(ch) = self.channels.get(self.selected) else { return };
        if ch.name.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("can't /react in the status buffer", now));
            return;
        }
        let net_id = ch.network_id;
        let target = ch.name.clone();
        let mut parts = rest.split_whitespace();
        let first = parts.next().unwrap_or("");
        let second = parts.next().unwrap_or("");
        if first.is_empty() {
            self.channels[self.selected].messages.push(system_line(
                "usage: /react <emoji>  or  /react <msgid> <emoji>",
                now,
            ));
            return;
        }
        let (msgid, emoji) = if second.is_empty() {
            // No msgid: react to the most recent message (any sender) with a msgid.
            let last_id = self.channels[self.selected]
                .messages
                .iter()
                .rev()
                .find_map(|m| m.msgid.clone());
            match last_id {
                Some(id) => (id, first.to_string()),
                None => {
                    self.channels[self.selected].messages.push(system_line(
                        "no recent message has a msgid to react to",
                        now,
                    ));
                    return;
                }
            }
        } else {
            (first.to_string(), second.to_string())
        };
        let _ = net_id; // silence unused
        if !self.send_out(
            Outgoing::React { target, msgid, emoji },
            now,
        ) {
            return;
        }
    }

    fn cmd_msgid(&mut self, rest: &str, now: Instant) {
        let needle = rest.trim().to_ascii_lowercase();
        let i = self.selected;
        let net_id = self.channels[i].network_id;
        let total = self.channels[i].messages.len();
        let with_id = self.channels[i].messages.iter().filter(|m| m.msgid.is_some()).count();
        let hit = self.channels[i]
            .messages
            .iter()
            .rev()
            .find(|m| {
                m.msgid.is_some()
                    && (needle.is_empty() || m.body.to_ascii_lowercase().contains(&needle))
            })
            .map(|m| (m.msgid.clone().unwrap(), m.nick.clone(), m.body.clone()));
        let line = match hit {
            Some((id, nick, body)) => {
                let preview: String = body.chars().take(60).collect();
                let suffix = if body.chars().count() > 60 { "…" } else { "" };
                format!("msgid: {id} — {nick}: {preview}{suffix}")
            }
            None => {
                let caps = self.net(net_id).map(|n| &n.caps_acked);
                let has = |c: &str| caps.is_some_and(|s| s.contains(c));
                let diag = format!(
                    "no msgid-bearing message ({with_id}/{total} in buffer) — caps: message-tags={} echo-message={} server-time={}",
                    if has("message-tags") { "✓" } else { "✗" },
                    if has("echo-message") { "✓" } else { "✗" },
                    if has("server-time") { "✓" } else { "✗" },
                );
                if needle.is_empty() { diag } else { format!("{diag} (filter: {:?})", needle) }
            }
        };
        self.channels[i].messages.push(system_line(&line, now));
    }

    fn cmd_caps(&mut self, now: Instant) {
        let i = self.selected;
        let net_id = self.channels[i].network_id;
        let mut caps: Vec<String> = self
            .net(net_id)
            .map(|n| n.caps_acked.iter().cloned().collect())
            .unwrap_or_default();
        caps.sort();
        let line = if caps.is_empty() {
            "no IRCv3 caps acked (or not connected)".to_string()
        } else {
            format!("caps acked ({}): {}", caps.len(), caps.join(" "))
        };
        self.channels[i].messages.push(system_line(&line, now));
        let filehost = self.net(net_id).and_then(|n| n.isupport.filehost.clone());
        let fh_line = match filehost {
            Some(url) => format!("FILEHOST: {url}"),
            None => "FILEHOST: not advertised".to_string(),
        };
        self.channels[i].messages.push(system_line(&fh_line, now));
    }

    fn cmd_history(&mut self, now: Instant) {
        // Window: last 7 days. The chathistory spec uses ISO8601 timestamps,
        // and `from < to` is required.
        let to_ts = chatlog::iso_now();
        let from_ts = chatlog::iso_minus_days(7);
        if !self.send_out(
            Outgoing::ChatHistoryTargets {
                from_ts: from_ts.clone(),
                to_ts: to_ts.clone(),
                limit: 50,
            },
            now,
        ) {
            return;
        }
        self.push_status_in(
            self.channels[self.selected].network_id,
            system_line(
                &format!("→ querying chathistory targets [{from_ts} .. {to_ts}]"),
                now,
            ),
        );
    }

    fn cmd_close(&mut self, now: Instant) {
        let i = self.selected;
        if i >= self.channels.len() {
            return;
        }
        let name = self.channels[i].name.clone();
        if name.starts_with('&') {
            self.channels[i]
                .messages
                .push(system_line("can't close the status buffer", now));
            return;
        }
        if name.starts_with('#') {
            let cid = self.channels[i].network_id;
            if let Some(tx) = self.net_mut(cid).and_then(|n| n.outgoing.as_mut()) {
                let _ = tx.try_send(Outgoing::Part { channel: name, reason: None });
            }
        }
        self.channels.remove(i);
        let new_sel = if i > 0 { i - 1 } else { 0 };
        self.set_selected(new_sel.min(self.channels.len().saturating_sub(1)));
    }

    fn cmd_nick(&mut self, rest: &str, now: Instant) {
        let new_nick = rest.split_whitespace().next().unwrap_or("");
        if new_nick.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /nick <newnick>", now));
            return;
        }
        self.send_out(Outgoing::Nick(new_nick.to_string()), now);
    }

    fn cmd_me(&mut self, rest: &str, now: Instant) {
        if rest.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /me <action>", now));
            return;
        }
        let target = self.channels[self.selected].name.clone();
        if target.starts_with('&') {
            self.channels[self.selected]
                .messages
                .push(system_line("can't /me in the status buffer", now));
            return;
        }
        let text = rest.to_string();
        if !self.send_out(
            Outgoing::Action { target: target.clone(), text: text.clone() },
            now,
        ) {
            return;
        }
        let nick = self.current_nickname().unwrap_or_else(|| "you".into());
        self.channels[self.selected].messages.push(ChatMessage {
            nick,
            body: text,
            time: now_hhmm(),
            day: "today".into(),
            inserted_at: now,
            mono_secs: now.elapsed().as_secs(),
            kind: MsgKind::Action,
            msgid: None, reactions: HashMap::new(),
        });
    }

    fn cmd_msg(&mut self, rest: &str, now: Instant) {
        let (target, body) = match rest.split_once(char::is_whitespace) {
            Some((t, b)) => (t.trim(), b.trim()),
            None => ("", ""),
        };
        if target.is_empty() || body.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /msg <target> <text>", now));
            return;
        }
        let target = target.to_string();
        let body = body.to_string();
        if !self.send_out(
            Outgoing::Privmsg { target: target.clone(), text: body.clone() },
            now,
        ) {
            return;
        }
        let idx = self.ensure_channel(&target);
        let nick = self.current_nickname().unwrap_or_else(|| "you".into());
        self.channels[idx].messages.push(ChatMessage {
            nick,
            body,
            time: now_hhmm(),
            day: "today".into(),
            inserted_at: now,
            mono_secs: now.elapsed().as_secs(),
            kind: MsgKind::Chat,
            msgid: None, reactions: HashMap::new(),
        });
        self.set_selected(idx);
    }

    fn cmd_query(&mut self, rest: &str, now: Instant) {
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /query <nick>", now));
            return;
        }
        let idx = self.ensure_channel(target);
        self.set_selected(idx);
        let _ = now;
    }

    fn cmd_logs(&mut self, now: Instant) {
        let server = self.server_for_log();
        let dir = directories::ProjectDirs::from("", "", "murmur")
            .map(|p| p.config_dir().join("logs").join(&server));
        let msg = match dir {
            Some(p) => format!("logs: {}", p.display()),
            None => "could not resolve logs directory".into(),
        };
        self.channels[self.selected]
            .messages
            .push(system_line(&msg, now));
    }

    fn cmd_server(&mut self, rest: &str, now: Instant) {
        let arg = rest.split_whitespace().next().unwrap_or("");
        if arg.is_empty() {
            let names: Vec<String> = self
                .networks
                .iter()
                .map(|n| {
                    let mark = if Some(n.id) == self.active { "* " } else { "  " };
                    format!("{mark}{}", n.cfg.name)
                })
                .collect();
            let body = if names.is_empty() {
                "no networks defined".into()
            } else {
                format!("networks:\n{}", names.join("\n"))
            };
            self.channels[self.selected]
                .messages
                .push(system_line(&body, now));
            return;
        }
        let target_id = self
            .networks
            .iter()
            .find(|n| n.cfg.name.eq_ignore_ascii_case(arg))
            .map(|n| n.id);
        match target_id {
            Some(id) => self.set_active_network(id),
            None => {
                self.channels[self.selected]
                    .messages
                    .push(system_line(&format!("no network named '{arg}'"), now));
            }
        }
    }

    fn cmd_connect(&mut self, rest: &str, now: Instant) {
        let arg = rest.split_whitespace().next().unwrap_or("");
        if arg.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /connect <name>", now));
            return;
        }
        let net_id = self
            .networks
            .iter()
            .find(|n| n.cfg.name.eq_ignore_ascii_case(arg))
            .map(|n| n.id);
        let Some(id) = net_id else {
            self.channels[self.selected]
                .messages
                .push(system_line(&format!("no network named '{arg}'"), now));
            return;
        };
        if let Some(net) = self.net_mut(id) {
            if net.autoconnect_enabled {
                let name = net.cfg.name.clone();
                self.push_status_in(id, system_line(&format!("{name} already enabled"), now));
                return;
            }
            net.autoconnect_enabled = true;
            net.status = ConnStatus::Connecting;
            let name = net.cfg.name.clone();
            self.push_status_in(id, system_line(&format!("connecting to {name}..."), now));
        }
    }

    fn cmd_disconnect(&mut self, rest: &str, now: Instant) {
        let arg = rest.split_whitespace().next().unwrap_or("");
        let target_id = if arg.is_empty() {
            self.active
        } else {
            self.networks
                .iter()
                .find(|n| n.cfg.name.eq_ignore_ascii_case(arg))
                .map(|n| n.id)
        };
        let Some(id) = target_id else {
            self.channels[self.selected]
                .messages
                .push(system_line(
                    if arg.is_empty() {
                        "no active network to disconnect"
                    } else {
                        "no such network"
                    },
                    now,
                ));
            return;
        };
        if let Some(net) = self.net_mut(id) {
            net.autoconnect_enabled = false;
            net.outgoing = None;
            net.status = ConnStatus::Disconnected;
            let name = net.cfg.name.clone();
            self.push_status_in(id, system_line(&format!("disconnected from {name}"), now));
        }
    }

    fn cmd_hide_joins(&mut self, now: Instant) {
        let ch = &mut self.channels[self.selected];
        ch.hide_joinpart = !ch.hide_joinpart;
        let msg = if ch.hide_joinpart {
            "join/part lines hidden in this channel"
        } else {
            "join/part lines visible in this channel"
        };
        ch.messages.push(system_line(msg, now));
    }

    fn cmd_ctcp(&mut self, rest: &str, now: Instant) {
        let mut parts = rest.splitn(3, char::is_whitespace);
        let target = parts.next().unwrap_or("").trim();
        let query = parts.next().unwrap_or("").trim().to_uppercase();
        let args = parts.next().unwrap_or("").trim();
        if target.is_empty() || query.is_empty() {
            self.channels[self.selected].messages.push(system_line(
                "usage: /ctcp <nick> <COMMAND> [args]",
                now,
            ));
            return;
        }
        let payload = if args.is_empty() {
            query.clone()
        } else {
            format!("{query} {args}")
        };
        if !self.send_out(
            Outgoing::Ctcp { target: target.into(), query: payload.clone() },
            now,
        ) {
            return;
        }
        let idx = self.ensure_channel(target);
        self.channels[idx]
            .messages
            .push(system_line(&format!("→ CTCP {payload} → {target}"), now));
    }

    fn cmd_ping(&mut self, rest: &str, now: Instant) {
        let target = rest.split_whitespace().next().unwrap_or("");
        if target.is_empty() {
            self.channels[self.selected]
                .messages
                .push(system_line("usage: /ping <nick>", now));
            return;
        }
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let payload = format!("PING {ts}");
        if !self.send_out(
            Outgoing::Ctcp { target: target.into(), query: payload },
            now,
        ) {
            return;
        }
        let idx = self.ensure_channel(target);
        self.channels[idx]
            .messages
            .push(system_line(&format!("→ ping → {target}"), now));
    }

    fn cmd_config(&mut self, rest: &str, now: Instant) {
        let arg = rest.split_whitespace().next().unwrap_or("");
        match arg {
            "" | "show" | "path" => {
                let msg = match config::config_path() {
                    Some(p) => format!("config: {}", p.display()),
                    None => "could not resolve config directory".into(),
                };
                self.channels[self.selected].messages.push(system_line(&msg, now));
                self.channels[self.selected]
                    .messages
                    .push(system_line("tip: /config template writes a fully-commented reference next to it", now));
            }
            "template" => match config::write_full_template_next_to_config() {
                Ok(p) => self
                    .channels[self.selected]
                    .messages
                    .push(system_line(&format!("wrote {}", p.display()), now)),
                Err(e) => self
                    .channels[self.selected]
                    .messages
                    .push(system_line(&format!("error: {e}"), now)),
            },
            other => {
                self.channels[self.selected]
                    .messages
                    .push(system_line(&format!("unknown subcommand: {other} (try /config or /config template)"), now));
            }
        }
    }

    fn cmd_theme(&mut self, rest: &str, now: Instant) {
        let arg = rest.split_whitespace().next().unwrap_or("");
        if arg.is_empty() {
            let names: Vec<&str> = themes::ALL.iter().map(|(n, _)| *n).collect();
            self.channels[self.selected].messages.push(system_line(
                &format!(
                    "current theme: {} · available: {}",
                    self.theme_name,
                    names.join(", ")
                ),
                now,
            ));
            return;
        }
        match themes::by_name(arg) {
            Some(p) => {
                theme::set(p);
                self.theme_name = arg.to_string();
                self.channels[self.selected]
                    .messages
                    .push(system_line(&format!("theme: {arg}"), now));
            }
            None => {
                self.channels[self.selected]
                    .messages
                    .push(system_line(&format!("unknown theme: {arg}"), now));
            }
        }
    }

    fn send_out(&mut self, msg: Outgoing, now: Instant) -> bool {
        let result = match self.active_net_mut().and_then(|n| n.outgoing.as_mut()) {
            Some(tx) => match tx.try_send(msg) {
                Ok(()) => Ok(()),
                Err(_) => Err("send failed — channel full or closed"),
            },
            None => Err("not connected"),
        };
        match result {
            Ok(()) => true,
            Err(reason) => {
                self.channels[self.selected]
                    .messages
                    .push(system_line(reason, now));
                false
            }
        }
    }

    fn set_active_network(&mut self, id: NetworkId) {
        if self.idx_of_net(id).is_none() {
            return;
        }
        // Remember current selection on the old active so we can restore it.
        if let Some(old_id) = self.active {
            let cur_sel = self.selected;
            if let Some(net) = self.net_mut(old_id) {
                net.last_selected = Some(cur_sel);
            }
        }
        self.active = Some(id);
        // Restore last selection in this network, or fall back to any
        // channel of this network (prefer &status).
        let restore = self
            .net(id)
            .and_then(|n| n.last_selected)
            .filter(|&i| {
                self.channels.get(i).is_some_and(|c| c.network_id == id)
            });
        let target = restore.or_else(|| {
            self.channels
                .iter()
                .position(|c| c.network_id == id && c.name == "&status")
        }).or_else(|| {
            self.channels.iter().position(|c| c.network_id == id)
        });
        if let Some(i) = target {
            self.set_selected(i);
        }
    }

    fn handle_irc(&mut self, network_id: NetworkId, event: IrcEvent) -> Task<Message> {
        let now = Instant::now();
        self.now = now;
        // Network must exist for the event to be meaningful — else drop it.
        if self.idx_of_net(network_id).is_none() {
            return Task::none();
        }
        let net_name = self
            .net(network_id)
            .map(|n| n.cfg.name.clone())
            .unwrap_or_default();
        match event {
            IrcEvent::Ready(tx) => {
                if let Some(net) = self.net_mut(network_id) {
                    net.outgoing = Some(tx);
                }
                Task::none()
            }
            IrcEvent::Connected => {
                if let Some(net) = self.net_mut(network_id) {
                    net.status = ConnStatus::Connected;
                }
                self.push_status_in(network_id, system_line("connected", now));
                Task::none()
            }
            IrcEvent::CapsAcked(caps) => {
                if let Some(net) = self.net_mut(network_id) {
                    net.caps_acked = caps.into_iter().collect();
                }
                Task::none()
            }
            IrcEvent::ConnectError(e) => {
                if let Some(net) = self.net_mut(network_id) {
                    net.status = ConnStatus::Error;
                    net.last_error = Some(e.clone());
                }
                self.push_status_in(network_id, system_line(&format!("error: {e}"), now));
                self.last_error = Some(e);
                Task::none()
            }
            IrcEvent::Disconnected => {
                if let Some(net) = self.net_mut(network_id) {
                    net.status = ConnStatus::Disconnected;
                }
                self.push_status_in(network_id, system_line("disconnected", now));
                Task::none()
            }
            IrcEvent::Reconnecting { in_secs: _ } => {
                // Worker is going to retry — keep the dot orange (Connecting)
                // rather than red, and clear the last error so we don't
                // misrepresent state. The Notice carrying the "reconnecting
                // in Ns…" line is emitted separately by the worker.
                if let Some(net) = self.net_mut(network_id) {
                    net.status = ConnStatus::Connecting;
                    net.last_error = None;
                }
                Task::none()
            }
            IrcEvent::Privmsg { target, nick, body, meta } => {
                if self.is_ignored(&nick) {
                    return Task::none();
                }
                let is_backlog = meta.batch_kind.as_deref() == Some("chathistory");
                let my_nick = self
                    .net(network_id)
                    .map(|n| n.cfg.nickname.clone())
                    .unwrap_or_default();
                let is_self = nick == my_nick;
                let is_dm = !my_nick.is_empty() && target == my_nick;
                let bucket = if is_dm { nick.clone() } else { target.clone() };
                if is_self
                    && !is_backlog
                    && self.dedupe_self_echo(network_id, &bucket, &body, MsgKind::Chat, &meta)
                {
                    return Task::none();
                }
                let viewing = self.is_actively_viewing(network_id, &bucket);
                let is_highlight = is_dm || self.is_highlight(&body, &my_nick);
                if !is_backlog && !is_self && !viewing {
                    if is_dm {
                        notify(format!("@{nick}"), body.clone());
                    } else if is_highlight {
                        notify(format!("{target} — {nick}"), body.clone());
                    }
                }
                let idx = self.ensure_channel_in(network_id, &bucket);
                // Mark unread/mention even for chathistory backlog: soju
                // replays missed-while-disconnected messages this way, and
                // those are still unread from the user's perspective. The
                // !viewing check keeps the currently-selected channel quiet.
                if !is_self && !viewing {
                    self.channels[idx].has_unread = true;
                    if is_highlight {
                        self.channels[idx].has_mention = true;
                    }
                    if self.channels[idx].read_marker_idx.is_none() {
                        self.channels[idx].read_marker_idx =
                            Some(self.channels[idx].messages.len());
                    }
                }
                let fetch = if is_backlog {
                    Task::none()
                } else {
                    self.schedule_media_fetches(&body)
                };
                if !is_backlog {
                    chatlog::append(
                        &net_name,
                        &bucket,
                        &format!("{}  <{}> {}", chatlog::iso_now(), nick, body),
                    );
                }
                self.channels[idx].messages.push(chat_line_from_meta(
                    nick,
                    body,
                    MsgKind::Chat,
                    &meta,
                    now,
                ));
                fetch
            }
            IrcEvent::Action { target, nick, body, meta } => {
                if self.is_ignored(&nick) {
                    return Task::none();
                }
                let is_backlog = meta.batch_kind.as_deref() == Some("chathistory");
                let my_nick = self
                    .net(network_id)
                    .map(|n| n.cfg.nickname.clone())
                    .unwrap_or_default();
                let is_self = nick == my_nick;
                let is_dm = !my_nick.is_empty() && target == my_nick;
                let bucket = if is_dm { nick.clone() } else { target.clone() };
                if is_self
                    && !is_backlog
                    && self.dedupe_self_echo(network_id, &bucket, &body, MsgKind::Action, &meta)
                {
                    return Task::none();
                }
                let viewing = self.is_actively_viewing(network_id, &bucket);
                let is_highlight = is_dm || self.is_highlight(&body, &my_nick);
                if !is_backlog && !is_self && !viewing {
                    if is_dm {
                        notify(format!("@{nick}"), format!("{nick} {body}"));
                    } else if is_highlight {
                        notify(format!("{target} — {nick}"), format!("{nick} {body}"));
                    }
                }
                let idx = self.ensure_channel_in(network_id, &bucket);
                if !is_self && !viewing {
                    self.channels[idx].has_unread = true;
                    if is_highlight {
                        self.channels[idx].has_mention = true;
                    }
                    if self.channels[idx].read_marker_idx.is_none() {
                        self.channels[idx].read_marker_idx =
                            Some(self.channels[idx].messages.len());
                    }
                }
                let fetch = if is_backlog {
                    Task::none()
                } else {
                    self.schedule_media_fetches(&body)
                };
                if !is_backlog {
                    chatlog::append(
                        &net_name,
                        &bucket,
                        &format!("{}  * {} {}", chatlog::iso_now(), nick, body),
                    );
                }
                self.channels[idx].messages.push(chat_line_from_meta(
                    nick,
                    body,
                    MsgKind::Action,
                    &meta,
                    now,
                ));
                fetch
            }
            IrcEvent::NickChanged { old, new, meta: _ } => {
                let is_self = self
                    .net(network_id)
                    .is_some_and(|n| n.cfg.nickname == old);
                if is_self {
                    if let Some(net) = self.net_mut(network_id) {
                        net.cfg.nickname = new.clone();
                    }
                }
                for ch in self.channels.iter_mut() {
                    if ch.network_id != network_id {
                        continue;
                    }
                    if let Some(pos) = ch.members.iter().position(|n| n == &old) {
                        ch.members[pos] = new.clone();
                        if let Some(meta) = ch.member_meta.remove(&old) {
                            ch.member_meta.insert(new.clone(), meta);
                        }
                        let body = if is_self {
                            format!("you are now {new}")
                        } else {
                            format!("{old} is now {new}")
                        };
                        ch.messages.push(joinpart_line(&body, now));
                    }
                }
                if is_self {
                    self.push_status_in(network_id, system_line(&format!("you are now {new}"), now));
                }
                Task::none()
            }
            IrcEvent::UserJoined { channel, nick, userhost, account, realname: _, meta } => {
                let idx = self.ensure_channel_in(network_id, &channel);
                if !self.channels[idx].members.iter().any(|n| n == &nick) {
                    self.channels[idx].members.push(nick.clone());
                }
                // Track whatever extra fields we can; prefixes will arrive
                // later via MODE / NAMES updates if applicable.
                let entry = self.channels[idx]
                    .member_meta
                    .entry(nick.clone())
                    .or_default();
                if userhost.is_some() {
                    entry.userhost = userhost;
                }
                // Stash extended-join account so it's available even if we
                // never see an account-notify or `account` tag later.
                if let (Some(net), Some(acc)) =
                    (self.net_mut(network_id), account.as_ref())
                {
                    net.accounts.insert(nick.to_ascii_lowercase(), acc.clone());
                }
                if meta.batch.is_none() {
                    chatlog::append(
                        &net_name,
                        &channel,
                        &format!("{}  -- {} joined", chatlog::iso_now(), nick),
                    );
                    let body = match account.as_deref() {
                        Some(a) if !a.eq_ignore_ascii_case(&nick) => {
                            format!("→ {nick} ({a}) joined")
                        }
                        _ => format!("→ {nick} joined"),
                    };
                    self.channels[idx]
                        .messages
                        .push(joinpart_line(&body, now));
                }
                let my_nick = self
                    .net(network_id)
                    .map(|n| n.cfg.nickname.clone())
                    .unwrap_or_default();
                if !my_nick.is_empty() && nick == my_nick {
                    self.maybe_fetch_initial_history(network_id, idx, &channel);
                }
                Task::none()
            }
            IrcEvent::AccountChanged { nick, account, meta: _ } => {
                let key = nick.to_ascii_lowercase();
                if let Some(net) = self.net_mut(network_id) {
                    match &account {
                        Some(a) => { net.accounts.insert(key, a.clone()); }
                        None => { net.accounts.remove(&key); }
                    }
                }
                let body = match account.as_deref() {
                    Some(a) => format!("-- {nick} is now logged in as {a}"),
                    None => format!("-- {nick} logged out of services"),
                };
                self.push_to_channels_with(network_id, &nick, &body, now);
                Task::none()
            }
            IrcEvent::AwayChanged { nick, message, meta: _ } => {
                let key = nick.to_ascii_lowercase();
                if let Some(net) = self.net_mut(network_id) {
                    if message.is_some() {
                        net.away_nicks.insert(key);
                    } else {
                        net.away_nicks.remove(&key);
                    }
                }
                Task::none()
            }
            IrcEvent::HostChanged { nick, ident, host, meta: _ } => {
                let body = format!("-- {nick} is now {ident}@{host}");
                self.push_to_channels_with(network_id, &nick, &body, now);
                Task::none()
            }
            IrcEvent::ISupport(snapshot) => {
                if let Some(net) = self.net_mut(network_id) {
                    net.isupport = snapshot;
                }
                Task::none()
            }
            IrcEvent::TypingChanged { target, nick, state } => {
                if self.is_ignored(&nick) {
                    return Task::none();
                }
                let bucket = self.bucket_for_target(network_id, &target, &nick);
                let key = (network_id, bucket, nick);
                match state {
                    irc_worker::TypingState::Active => {
                        self.typing_observed.insert(key, now);
                    }
                    _ => {
                        self.typing_observed.remove(&key);
                    }
                }
                Task::none()
            }
            IrcEvent::ReadMarker { target, timestamp } => {
                let key = (network_id, target);
                if let Some(ts) = timestamp {
                    self.read_markers.insert(key, ts);
                } else {
                    self.read_markers.remove(&key);
                }
                Task::none()
            }
            IrcEvent::Redacted { target, msgid, by_nick, reason } => {
                let bucket = self.bucket_for_target(network_id, &target, &by_nick);
                if let Some(idx) = self.find_channel_in(network_id, &bucket) {
                    if let Some(entry) = self.channels[idx]
                        .messages
                        .iter_mut()
                        .find(|m| m.msgid.as_deref() == Some(msgid.as_str()))
                    {
                        let suffix = match reason.as_deref() {
                            Some(r) if !r.is_empty() => format!(" ({r})"),
                            _ => String::new(),
                        };
                        entry.body = format!("[deleted by {by_nick}{suffix}]");
                        entry.kind = MsgKind::System;
                    }
                }
                Task::none()
            }
            IrcEvent::Reaction { target, target_msgid, nick, emoji } => {
                if self.is_ignored(&nick) {
                    return Task::none();
                }
                let bucket = self.bucket_for_target(network_id, &target, &nick);
                if let Some(idx) = self.find_channel_in(network_id, &bucket) {
                    if let Some(entry) = self.channels[idx]
                        .messages
                        .iter_mut()
                        .find(|m| m.msgid.as_deref() == Some(target_msgid.as_str()))
                    {
                        entry.reactions
                            .entry(emoji)
                            .or_default()
                            .insert(nick);
                    }
                }
                Task::none()
            }
            IrcEvent::UserLeft { channel, nick, meta } => {
                let idx = self.ensure_channel_in(network_id, &channel);
                self.channels[idx].members.retain(|n| n != &nick);
                self.channels[idx].member_meta.remove(&nick);
                if meta.batch.is_none() {
                    chatlog::append(
                        &net_name,
                        &channel,
                        &format!("{}  -- {} left", chatlog::iso_now(), nick),
                    );
                    self.channels[idx]
                        .messages
                        .push(joinpart_line(&format!("← {nick} left"), now));
                }
                Task::none()
            }
            IrcEvent::Names { channel, members } => {
                let idx = self.ensure_channel_in(network_id, &channel);
                for entry in members {
                    if !self.channels[idx].members.iter().any(|m| m == &entry.nick) {
                        self.channels[idx].members.push(entry.nick.clone());
                    }
                    self.channels[idx].member_meta.insert(
                        entry.nick.clone(),
                        MemberMeta {
                            prefixes: entry.prefixes,
                            userhost: entry.userhost,
                        },
                    );
                }
                Task::none()
            }
            IrcEvent::Topic { channel, topic } => {
                let idx = self.ensure_channel_in(network_id, &channel);
                chatlog::append(
                    &net_name,
                    &channel,
                    &format!("{}  -- topic: {}", chatlog::iso_now(), topic),
                );
                self.channels[idx].topic = Some(topic);
                Task::none()
            }
            IrcEvent::Notice { from, text, meta: _ } => {
                if from != "*" && self.is_ignored(&from) {
                    return Task::none();
                }
                self.push_status_in(network_id, system_line(&format!("-{from}- {text}"), now));
                Task::none()
            }
            IrcEvent::CtcpReply { from, query, args } => {
                let body = if query.eq_ignore_ascii_case("PING") {
                    if let Ok(sent) = args.trim().parse::<u128>() {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis())
                            .unwrap_or(0);
                        let latency = now_ms.saturating_sub(sent);
                        format!("← pong from {from}: {latency}ms")
                    } else {
                        format!("← PING reply from {from}: {args}")
                    }
                } else if args.is_empty() {
                    format!("← CTCP {query} reply from {from}")
                } else {
                    format!("← CTCP {query} reply from {from}: {args}")
                };
                let idx = self.ensure_channel_in(network_id, &from);
                self.channels[idx].messages.push(system_line(&body, now));
                Task::none()
            }
        }
    }

    // Resolve a channel name in the *active* network, creating it if needed.
    // Used by all command paths (commands act on the active network).
    fn ensure_channel(&mut self, name: &str) -> usize {
        let id = self.active.unwrap_or(NetworkId(0));
        self.ensure_channel_in(id, name)
    }

    fn set_selected(&mut self, i: usize) {
        if self.selected != i || self.channels[i].fade_baseline.elapsed().as_millis() > FADE_MS * 2 {
            self.channels[i].fade_baseline = Instant::now();
        }
        if self.selected != i {
            if let Some(prev) = self.channels.get_mut(self.selected) {
                prev.read_marker_idx = None;
            }
        }
        self.selected = i;
        if self.window_focused {
            self.channels[i].has_unread = false;
            self.channels[i].has_mention = false;
            self.send_read_marker();
        }
    }

    // Push a MARKREAD for the currently-selected target with the timestamp
    // of the most recent message in the buffer. No-op when the server
    // hasn't ACKed `draft/read-marker` or when in &status.
    fn send_read_marker(&mut self) {
        let Some(ch) = self.channels.get(self.selected) else { return };
        if ch.name.starts_with('&') {
            return;
        }
        let supported = self
            .net(ch.network_id)
            .is_some_and(|n| n.caps_acked.contains("draft/read-marker"));
        if !supported {
            return;
        }
        // Use the latest message's mono_secs for an approximate ISO
        // timestamp. Without a server-time on every msg we can only
        // approximate with our own clock.
        let timestamp = chatlog::iso_now();
        let net_id = ch.network_id;
        let target = ch.name.clone();
        if let Some(tx) = self.net_mut(net_id).and_then(|n| n.outgoing.as_mut()) {
            let _ = tx.try_send(Outgoing::MarkRead {
                target,
                timestamp: Some(timestamp),
            });
        }
    }

    fn clear_active_unread(&mut self) {
        if self.window_focused
            && self.selected < self.channels.len()
        {
            self.channels[self.selected].has_unread = false;
            self.channels[self.selected].has_mention = false;
        }
    }

    fn maybe_fetch_initial_history(
        &mut self,
        network_id: NetworkId,
        ch_idx: usize,
        target: &str,
    ) {
        if self.channels[ch_idx].chathistory_requested {
            return;
        }
        let supported = self
            .net(network_id)
            .is_some_and(|n| n.caps_acked.contains("draft/chathistory"));
        if !supported {
            return;
        }
        if let Some(tx) = self.net_mut(network_id).and_then(|n| n.outgoing.as_mut()) {
            let _ = tx.try_send(Outgoing::ChatHistoryLatest {
                target: target.to_string(),
                limit: 50,
            });
            self.channels[ch_idx].chathistory_requested = true;
        }
    }

    fn is_actively_viewing(&self, net_id: NetworkId, bucket: &str) -> bool {
        self.window_focused
            && self.active == Some(net_id)
            && self
                .channels
                .get(self.selected)
                .is_some_and(|c| c.network_id == net_id && c.name == bucket)
    }

    fn is_ignored(&self, nick: &str) -> bool {
        !nick.is_empty() && self.ignored_nicks.contains(&nick.to_ascii_lowercase())
    }

    // When `echo-message` is acked, the server echoes our own PRIVMSGs
    // back. We've already inserted them locally on send, so look back a
    // few entries and graft the server-assigned msgid onto the local row,
    // then tell the caller to skip the duplicate insert.
    //
    // Returns true if the echo was deduped (drop it) and false if the
    // event should be inserted normally (e.g. multi-device: another client
    // sent the message and we're seeing it for the first time).
    fn dedupe_self_echo(
        &mut self,
        network_id: NetworkId,
        bucket: &str,
        body: &str,
        kind: MsgKind,
        meta: &irc_worker::MsgMeta,
    ) -> bool {
        let Some(net) = self.net(network_id) else { return false };
        if !net.caps_acked.contains("echo-message") {
            return false;
        }
        let my_nick = net.cfg.nickname.clone();
        if my_nick.is_empty() {
            return false;
        }
        let Some(idx) = self.find_channel_in(network_id, bucket) else { return false };
        let messages = &mut self.channels[idx].messages;
        let scan = messages.len().min(30);
        let start = messages.len() - scan;
        for entry in messages[start..].iter_mut().rev() {
            if entry.nick == my_nick
                && entry.kind == kind
                && entry.msgid.is_none()
                && entry.body == body
            {
                entry.msgid = meta.msgid.clone();
                return true;
            }
        }
        false
    }

    // Channel-name prefix characters advertised by the active server via
    // ISUPPORT (`CHANTYPES=`). Falls back to `#&` per RFC2812.
    fn active_chantypes(&self) -> String {
        let from_isupport = self
            .active_net()
            .map(|n| n.isupport.chantypes.clone())
            .filter(|s| !s.is_empty());
        from_isupport.unwrap_or_else(|| "#&".into())
    }

    // Returns the highest-priority channel prefix character for a nick
    // in the currently-selected channel (e.g. `@`, `+`), or None.
    fn member_prefix(&self, nick: &str) -> Option<char> {
        let ch = self.channels.get(self.selected)?;
        ch.member_meta
            .get(nick)
            .and_then(|m| m.prefixes.chars().next())
    }

    // True when the nick is marked away on the network owning the
    // currently-selected channel. Used to fade entries in the member list.
    fn is_nick_away(&self, nick: &str) -> bool {
        if nick.is_empty() {
            return false;
        }
        let Some(ch) = self.channels.get(self.selected) else { return false };
        let key = nick.to_ascii_lowercase();
        self.net(ch.network_id)
            .is_some_and(|n| n.away_nicks.contains(&key))
    }

    fn is_highlight(&self, body: &str, my_nick: &str) -> bool {
        if !my_nick.is_empty() && mentions(body, my_nick) {
            return true;
        }
        self.highlight_keywords.iter().any(|kw| mentions(body, kw))
    }

    fn sync_channel_animations(&mut self, now: Instant) {
        for (i, ch) in self.channels.iter_mut().enumerate() {
            let hovered = self.hovered_channel == Some(i);
            let selected = self.selected == i;
            if ch.hover_anim.value() != hovered {
                ch.hover_anim.go_mut(hovered, now);
            }
            if ch.select_anim.value() != selected {
                ch.select_anim.go_mut(selected, now);
            }
        }
    }

    fn push_status_in(&mut self, network_id: NetworkId, msg: ChatMessage) {
        let idx = self.ensure_channel_in(network_id, "&status");
        self.channels[idx].messages.push(msg);
    }

    // Emit a system line in every channel on `network_id` where `nick`
    // is a member, plus the network's &status buffer as a catch-all.
    // Used for ACCOUNT / CHGHOST broadcasts that affect a user globally.
    fn push_to_channels_with(
        &mut self,
        network_id: NetworkId,
        nick: &str,
        body: &str,
        now: Instant,
    ) {
        let mut hit_any = false;
        for ch in self.channels.iter_mut() {
            if ch.network_id != network_id {
                continue;
            }
            if ch.members.iter().any(|n| n == nick) {
                ch.messages.push(joinpart_line(body, now));
                hit_any = true;
            }
        }
        if !hit_any {
            self.push_status_in(network_id, joinpart_line(body, now));
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let now = Instant::now();
        let msg_anim = self.channels.iter().any(|c| {
            c.fade_baseline.elapsed().as_millis() < FADE_MS
                || c.messages
                    .iter()
                    .any(|m| m.inserted_at.elapsed().as_millis() < FADE_MS)
        });
        let pane_anim =
            self.sidebar_anim.is_animating(now) || self.members_anim.is_animating(now);
        let row_anim = self
            .channels
            .iter()
            .any(|c| c.hover_anim.is_animating(now) || c.select_anim.is_animating(now));

        let mut subs: Vec<Subscription<Message>> = Vec::new();
        if msg_anim || pane_anim || row_anim {
            subs.push(window::frames().map(Message::Tick));
        }
        for net in &self.networks {
            if !net.autoconnect_enabled {
                continue;
            }
            let key = (net.id, net.cfg.clone());
            subs.push(Subscription::run_with(key, irc_sub_for_network));
        }
        subs.push(keyboard::listen().map(Message::Key));
        subs.push(window::events().filter_map(|(_id, ev)| match ev {
            window::Event::Focused => Some(Message::WindowFocus(true)),
            window::Event::Unfocused => Some(Message::WindowFocus(false)),
            window::Event::FileHovered(_) => Some(Message::FileHovered),
            window::Event::FilesHoveredLeft => Some(Message::FilesHoveredLeft),
            window::Event::FileDropped(path) => Some(Message::FileDropped(path)),
            _ => None,
        }));

        Subscription::batch(subs)
    }

    fn sidebar_target_width(&self) -> f32 {
        let mut longest_chan: usize = 6;
        for ch in &self.channels {
            if Some(ch.network_id) == self.active {
                let (_, label) = channel_parts(&ch.name);
                let chars = truncate(&label, 18).chars().count();
                longest_chan = longest_chan.max(chars);
            }
        }
        let mut longest_net: usize = 0;
        for n in &self.networks {
            let chars = truncate(&n.cfg.name, 14).chars().count();
            longest_net = longest_net.max(chars);
        }
        let chan_w = sz(13.0) * 0.62 * (longest_chan as f32 + 1.0) + 14.0; // +# prefix
        let net_w = sz(11.0) * 0.7 * (longest_net as f32) + 16.0; // dot + spacing
        let w = chan_w.max(net_w) + 16.0 + 12.0; // button h-pad + section h-pad
        w.clamp(SIDEBAR_MIN_W, SIDEBAR_MAX_W)
    }

    fn members_target_width(&self) -> f32 {
        let mut longest: usize = 6;
        if let Some(ch) = self.channels.get(self.selected) {
            for nick in &ch.members {
                let chars = truncate(nick, 14).chars().count();
                longest = longest.max(chars);
            }
        }
        let char_w = sz(12.0) * 0.62;
        // dot (6) + spacing (S2=6) + label + paddings
        let w = 6.0 + 6.0 + (longest as f32 * char_w) + 24.0;
        w.clamp(MEMBERS_MIN_W, MEMBERS_MAX_W)
    }

    fn view(&self) -> Element<'_, Message> {
        let sidebar_target = self.sidebar_target_width();
        let members_target = self.members_target_width();
        let sw = self.sidebar_anim.interpolate(0.0, sidebar_target, self.now);
        let mw = self.members_anim.interpolate(0.0, members_target, self.now);

        let mut panes: Vec<Element<Message>> = Vec::with_capacity(3);
        if sw > 0.5 {
            panes.push(self.sidebar(sw, sidebar_target));
        }
        panes.push(self.chat_pane());
        if mw > 0.5 {
            panes.push(self.member_pane(mw, members_target));
        }

        let main: Element<Message> = row(panes).spacing(0).height(Fill).into();

        // Overlay stack: settings goes on top of palette so Cmd+, while
        // palette is open lands the user on settings without losing state.
        if self.settings_open {
            stack![main, self.settings_overlay()].into()
        } else if self.palette_open {
            stack![main, self.palette_overlay()].into()
        } else if self.emoji_picker.is_some() {
            stack![main, self.emoji_picker_overlay()].into()
        } else {
            main
        }
    }

    fn palette_overlay(&self) -> Element<'_, Message> {
        let items = self.filtered_palette_items();

        let input = text_input("type to search, or /command …", &self.palette_query)
            .id(PALETTE_INPUT_ID)
            .on_input(Message::PaletteQuery)
            .on_submit(Message::PaletteActivate)
            .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
            .size(sz(14.0))
            .style(palette_input_style);

        let rows: Vec<Element<Message>> = items
            .into_iter()
            .enumerate()
            .map(|(i, item)| self.palette_row(i, item))
            .collect();

        let list: Element<Message> = if rows.is_empty() {
            container(
                text(if self.palette_query.trim().starts_with('/') {
                    "press enter to run this command"
                } else {
                    "no matches"
                })
                .size(sz(12.0))
                .color(tok::text_muted()),
            )
            .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
            .into()
        } else {
            // Scrollable so command/channel lists longer than the
            // viewport stay reachable with mouse wheel. Arrow keys still
            // move the cursor through every item; auto-scroll to the
            // highlighted row is a separate refinement.
            scrollable(column(rows).spacing(1))
                .height(Length::Fixed(PALETTE_MAX_ITEMS as f32 * 34.0))
                .into()
        };

        let divider = container(sp(Fill, 1)).style(|_| container::Style {
            background: Some(Background::Color(tok::border_soft())),
            ..Default::default()
        });

        let modal = container(column![input, divider, list].spacing(0))
            .width(Length::Fixed(PALETTE_W))
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_1())),
                border: Border {
                    color: tok::border(),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                shadow: Shadow {
                    color: Color { a: 0.45, ..Color::BLACK },
                    offset: iced::Vector::new(0.0, 12.0),
                    blur_radius: 40.0,
                },
                ..Default::default()
            })
            .clip(true);

        let backdrop = mouse_area(
            container(Space::new().width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color { a: 0.35, ..Color::BLACK })),
                    ..Default::default()
                }),
        )
        .on_press(Message::PaletteClose);

        let centered = container(modal)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .padding(pad(90.0, 0.0, 0.0, 0.0));

        stack![backdrop, centered].into()
    }

    fn emoji_picker_overlay(&self) -> Element<'_, Message> {
        let st = self.emoji_picker.as_ref().expect("emoji_picker_overlay called when picker closed");

        let search = text_input("search…", &st.query)
            .id(EMOJI_PICKER_INPUT_ID)
            .on_input(Message::EmojiPickerQuery)
            .size(sz(12.0))
            .padding(pad(tok::S2, tok::S3, tok::S2, tok::S3))
            .style(palette_input_style);

        // Build the emoji grid: 8 columns of clickable buttons.
        let entries: Vec<&'static emoji::Entry> =
            emoji::filter(st.category, &st.query).take(400).collect();

        const COLS: usize = 8;
        let mut rows_vec: Vec<Element<Message>> = Vec::new();
        for chunk in entries.chunks(COLS) {
            let mut cells: Vec<Element<Message>> = Vec::with_capacity(COLS);
            for entry in chunk {
                cells.push(emoji_cell(entry).into());
            }
            // Pad incomplete row so cells stay left-aligned.
            while cells.len() < COLS {
                cells.push(sp(EMOJI_CELL, EMOJI_CELL).into());
            }
            rows_vec.push(row(cells).spacing(2).into());
        }

        let grid: Element<Message> = if entries.is_empty() {
            container(
                text("no matches")
                    .size(sz(12.0))
                    .color(tok::text_muted()),
            )
            .padding(pad(tok::S4, tok::S4, tok::S4, tok::S4))
            .into()
        } else {
            scrollable(
                column(rows_vec)
                    .spacing(2)
                    .padding(pad(tok::S2, tok::S2, tok::S2, tok::S2)),
            )
            .height(Length::Fixed(260.0))
            .into()
        };

        // Category tabs at the bottom.
        let cat_tabs: Vec<Element<Message>> = emoji::Category::ALL
            .iter()
            .map(|c| {
                let selected = st.category == Some(*c);
                let glyph = c.glyph();
                mouse_area(
                    button(
                        container(text(glyph).size(sz(14.0)))
                            .width(Length::Fixed(28.0))
                            .height(Length::Fixed(28.0))
                            .align_x(iced::alignment::Horizontal::Center)
                            .align_y(iced::alignment::Vertical::Center),
                    )
                    .on_press(Message::EmojiPickerCategory(*c))
                    .padding(0)
                    .style(move |_theme, status| {
                        let bg = if selected {
                            tok::accent_soft()
                        } else {
                            match status {
                                button::Status::Hovered => tok::bg_hover(),
                                _ => Color::TRANSPARENT,
                            }
                        };
                        button::Style {
                            background: Some(Background::Color(bg)),
                            text_color: tok::text(),
                            border: Border { radius: 6.0.into(), ..Default::default() },
                            shadow: Shadow::default(),
                            ..Default::default()
                        }
                    }),
                )
                .interaction(iced::mouse::Interaction::Pointer)
                .into()
            })
            .collect();

        let cat_bar = container(row(cat_tabs).spacing(2).align_y(iced::Alignment::Center))
            .padding(pad(tok::S2, tok::S3, tok::S2, tok::S3))
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_2())),
                ..Default::default()
            });

        let divider = container(sp(Fill, 1)).style(|_| container::Style {
            background: Some(Background::Color(tok::border_soft())),
            ..Default::default()
        });

        let modal = container(column![search, divider, grid, cat_bar].spacing(0))
            .width(Length::Fixed(360.0))
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_1())),
                border: Border {
                    color: tok::border(),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                shadow: Shadow {
                    color: Color { a: 0.45, ..Color::BLACK },
                    offset: iced::Vector::new(0.0, 12.0),
                    blur_radius: 40.0,
                },
                ..Default::default()
            })
            .clip(true);

        let backdrop = mouse_area(
            container(Space::new().width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color { a: 0.0, ..Color::BLACK })),
                    ..Default::default()
                }),
        )
        .on_press(Message::EmojiPickerClose);

        // Anchor near the bottom-right (above the compose bar).
        let anchored = container(modal)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Bottom)
            .padding(pad(0.0, tok::S4 as f32 + 50.0, 60.0, 0.0));

        stack![backdrop, anchored].into()
    }

    fn settings_overlay(&self) -> Element<'_, Message> {
        let header_label = text("settings")
            .size(sz(13.0))
            .font(medium())
            .color(tok::text());
        let close_btn = button(
            text("×")
                .size(sz(16.0))
                .color(tok::text_muted())
                .font(medium()),
        )
        .padding(pad(0.0, tok::S3, 0.0, tok::S3))
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsClose);
        let header = container(
            row![header_label, sp(Fill, 0), close_btn]
                .align_y(iced::Alignment::Center)
                .spacing(tok::S3),
        )
        .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
        .width(Fill);

        let header_divider = container(sp(Fill, 1)).style(|_| container::Style {
            background: Some(Background::Color(tok::border_soft())),
            ..Default::default()
        });

        let nav = column![
            self.settings_section_button("Appearance", SettingsSection::Appearance),
            self.settings_section_button("Notifications", SettingsSection::Notifications),
            self.settings_section_button("Networks", SettingsSection::Networks),
            self.settings_section_button("Upload", SettingsSection::Upload),
        ]
        .spacing(tok::S1)
        .width(Length::Fixed(150.0));

        let section: Element<Message> = match self.settings_section {
            SettingsSection::Appearance => self.settings_appearance_section(),
            SettingsSection::Notifications => self.settings_notifications_section(),
            SettingsSection::Networks => self.settings_networks_section(),
            SettingsSection::Upload => self.settings_upload_section(),
        };
        let section_scroll = scrollable(
            container(section)
                .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
                .width(Fill),
        )
        .height(Fill);

        let body = row![
            container(nav)
                .padding(pad(tok::S3, tok::S3, tok::S3, tok::S3))
                .style(|_| container::Style {
                    background: Some(Background::Color(tok::bg_0())),
                    ..Default::default()
                })
                .height(Fill),
            container(sp(1, Fill)).style(|_| container::Style {
                background: Some(Background::Color(tok::border_soft())),
                ..Default::default()
            }),
            section_scroll,
        ]
        .height(Length::Fixed(440.0));

        let footer_divider = container(sp(Fill, 1)).style(|_| container::Style {
            background: Some(Background::Color(tok::border_soft())),
            ..Default::default()
        });
        let status_label: Element<Message> =
            if let Some(err) = self.settings_save_error.as_deref() {
                text(err.to_string())
                    .size(sz(11.0))
                    .color(Color { a: 1.0, ..tok::accent() })
                    .into()
            } else if let Some(info) = self.settings_save_info.as_deref() {
                text(info.to_string())
                    .size(sz(11.0))
                    .color(tok::text_muted())
                    .into()
            } else {
                sp(0, 0).into()
            };
        let save_btn = button(
            text("Save")
                .size(sz(12.0))
                .font(medium())
                .color(Color::WHITE),
        )
        .padding(pad(tok::S2 as f32, tok::S4, tok::S2 as f32, tok::S4))
        .style(|_, status| primary_btn_style(status))
        .on_press(Message::SettingsSave);
        let cancel_btn = button(
            text("Close")
                .size(sz(12.0))
                .font(medium())
                .color(tok::text_mid()),
        )
        .padding(pad(tok::S2 as f32, tok::S4, tok::S2 as f32, tok::S4))
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsClose);
        let footer = container(
            row![status_label, sp(Fill, 0), cancel_btn, save_btn]
                .spacing(tok::S2)
                .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
        .width(Fill);

        let modal = container(
            column![header, header_divider, body, footer_divider, footer].spacing(0),
        )
        .width(Length::Fixed(720.0))
        .style(|_| container::Style {
            background: Some(Background::Color(tok::bg_1())),
            border: Border {
                color: tok::border(),
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: Color { a: 0.45, ..Color::BLACK },
                offset: iced::Vector::new(0.0, 12.0),
                blur_radius: 40.0,
            },
            ..Default::default()
        })
        .clip(true);

        let backdrop = mouse_area(
            container(Space::new().width(Fill).height(Fill))
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(Color { a: 0.45, ..Color::BLACK })),
                    ..Default::default()
                }),
        )
        .on_press(Message::SettingsClose);

        let centered = container(modal)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .padding(pad(60.0, 0.0, 0.0, 0.0));

        stack![backdrop, centered].into()
    }

    fn settings_section_button(
        &self,
        label: &'static str,
        section: SettingsSection,
    ) -> Element<'_, Message> {
        let selected = self.settings_section == section;
        let label_color = if selected { tok::text() } else { tok::text_muted() };
        let content = row![text(label)
            .size(sz(12.0))
            .font(medium())
            .color(label_color),]
        .align_y(iced::Alignment::Center)
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3));
        button(content)
            .on_press(Message::SettingsSelectSection(section))
            .width(Fill)
            .padding(0)
            .style(move |_, status| section_btn_style(selected, status))
            .into()
    }

    fn settings_appearance_section(&self) -> Element<'_, Message> {
        let theme_names: Vec<String> =
            themes::ALL.iter().map(|(n, _)| (*n).to_string()).collect();
        let current_theme = self
            .settings_draft
            .theme
            .clone()
            .unwrap_or_else(|| "soft-dark".to_string());
        let theme_picker = pick_list(theme_names, Some(current_theme), |s: String| {
            Message::SettingsThemeChanged(s)
        })
        .text_size(sz(12.0))
        .width(Length::Fixed(220.0));

        let font_family_value = self
            .settings_draft
            .font_family
            .clone()
            .unwrap_or_default();
        let font_input = text_input("e.g. JetBrains Mono (empty = bundled)", &font_family_value)
            .on_input(Message::SettingsFontFamily)
            .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
            .size(sz(12.0))
            .width(Length::Fixed(320.0))
            .style(|_, status| input_style(status));

        let scale_value = self.settings_draft.font_size_scale.unwrap_or(1.0);
        let scale_slider = slider(0.5..=3.0, scale_value, Message::SettingsFontScale).step(0.05);
        let scale_label = text(format!("{scale_value:.2}×"))
            .size(sz(11.0))
            .color(tok::text_mid())
            .width(Length::Fixed(54.0));
        let scale_row = row![
            container(scale_slider).width(Length::Fixed(266.0)),
            scale_label
        ]
        .spacing(tok::S3)
        .align_y(iced::Alignment::Center);

        let note = text("Theme applies instantly. Font family and font scale need a restart.")
            .size(sz(10.0))
            .color(tok::text_faint());

        column![
            settings_section_header("Appearance"),
            settings_row("Theme", theme_picker.into()),
            settings_row("Font family", font_input.into()),
            settings_row("Font scale", scale_row.into()),
            sp(0, tok::S2),
            note,
        ]
        .spacing(tok::S3)
        .into()
    }

    fn settings_notifications_section(&self) -> Element<'_, Message> {
        let chips: Vec<Element<Message>> = self
            .settings_draft
            .highlight_keywords
            .iter()
            .enumerate()
            .map(|(i, kw)| chip(kw, Message::SettingsKwRemove(i)))
            .collect();
        let chip_row: Element<Message> = if chips.is_empty() {
            text("no keywords yet — your own nick is always a highlight")
                .size(sz(11.0))
                .color(tok::text_faint())
                .into()
        } else {
            wrap_row(chips, tok::S2 as f32)
        };

        let input = text_input("add a keyword and press Enter", &self.settings_kw_input)
            .on_input(Message::SettingsKwInput)
            .on_submit(Message::SettingsKwAdd)
            .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
            .size(sz(12.0))
            .width(Length::Fixed(300.0))
            .style(|_, status| input_style(status));
        let add_btn = button(
            text("+")
                .size(sz(14.0))
                .font(medium())
                .color(tok::text_mid()),
        )
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsKwAdd);

        let note = text(
            "Words that, when they appear in any channel you're not actively reading, \
trigger an OS notification and mark the channel as a mention. Case-insensitive, \
matched on word boundaries.",
        )
        .size(sz(10.0))
        .color(tok::text_faint());

        column![
            settings_section_header("Notifications"),
            text("Highlight keywords")
                .size(sz(11.0))
                .font(medium())
                .color(tok::text_mid()),
            chip_row,
            row![input, add_btn]
                .spacing(tok::S2)
                .align_y(iced::Alignment::Center),
            sp(0, tok::S2),
            note,
        ]
        .spacing(tok::S3)
        .into()
    }

    fn settings_upload_section(&self) -> Element<'_, Message> {
        let up = &self.settings_draft.upload;
        let use_custom = up.use_custom;

        let toggle: Element<Message> = checkbox(use_custom)
            .label("Use a custom uploader (pastebin / HTTP) instead of the server")
            .on_toggle(Message::SettingsUploadUseCustom)
            .text_size(sz(11.0))
            .size(14.0)
            .into();

        let intro = text(
            "Where the attach button sends files. By default it uses the IRC server's \
advertised FILEHOST endpoint; enable a custom uploader to use your own service \
(e.g. a pastebin REST API).",
        )
        .size(sz(10.0))
        .color(tok::text_faint());

        let mut col = column![settings_section_header("Upload"), intro, toggle].spacing(tok::S3);

        if use_custom {
            let c = up.custom.clone().unwrap_or_default();
            let url_input = text_input("https://paste.example.com/api/upload", &c.url)
                .on_input(|v| Message::SettingsUploadField(UploadField::Url, v))
                .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
                .size(sz(11.0))
                .width(Length::Fixed(360.0))
                .style(|_, status| input_style(status));
            let token_input =
                text_input("optional — sent as Authorization: Bearer", &c.token.clone().unwrap_or_default())
                    .on_input(|v| Message::SettingsUploadField(UploadField::Token, v))
                    .secure(true)
                    .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
                    .size(sz(11.0))
                    .width(Length::Fixed(360.0))
                    .style(|_, status| input_style(status));
            let field_input = text_input("file  (empty = raw body)", &c.field)
                .on_input(|v| Message::SettingsUploadField(UploadField::Field, v))
                .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
                .size(sz(11.0))
                .width(Length::Fixed(220.0))
                .style(|_, status| input_style(status));
            let kinds = vec!["json".to_string(), "location".to_string(), "text".to_string()];
            let kind_pick = pick_list(kinds, Some(c.response_kind.clone()), Message::SettingsUploadKind)
                .text_size(sz(11.0))
                .width(Length::Fixed(140.0));
            let key_input = text_input("url", &c.response_key)
                .on_input(|v| Message::SettingsUploadField(UploadField::ResponseKey, v))
                .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
                .size(sz(11.0))
                .width(Length::Fixed(140.0))
                .style(|_, status| input_style(status));

            col = col.push(settings_row("Upload URL", url_input.into()));
            col = col.push(settings_row("Bearer token", token_input.into()));
            col = col.push(settings_row("Multipart field", field_input.into()));
            col = col.push(settings_row("Response", kind_pick.into()));
            let kind = c.response_kind.trim();
            if kind == "json" || kind.is_empty() {
                col = col.push(settings_row("JSON key", key_input.into()));
            }
            col = col.push(
                text(
                    "Tip: for inline image previews, use the response key that returns the \
direct/raw file URL.",
                )
                .size(sz(10.0))
                .color(tok::text_faint()),
            );
        } else {
            let status = self
                .channels
                .get(self.selected)
                .and_then(|ch| self.net(ch.network_id))
                .and_then(|n| n.isupport.filehost.clone());
            let line = match status {
                Some(u) => format!("Server FILEHOST: {u}"),
                None => "Server FILEHOST: not advertised by the current network".to_string(),
            };
            col = col.push(text(line).size(sz(11.0)).color(tok::text_mid()));
        }

        col = col.push(sp(0, tok::S2));
        col = col.push(
            text("Saved on Save · applies immediately.")
                .size(sz(10.0))
                .color(tok::text_faint()),
        );
        col.into()
    }

    fn settings_networks_section(&self) -> Element<'_, Message> {
        let list_items: Vec<Element<Message>> = self
            .settings_draft
            .networks
            .iter()
            .enumerate()
            .map(|(i, n)| self.settings_network_list_row(i, &n.name))
            .collect();
        let list_col = if list_items.is_empty() {
            column![text("no networks defined")
                .size(sz(11.0))
                .color(tok::text_faint())]
        } else {
            column(list_items).spacing(2)
        };

        let add_btn = button(
            text("+ add network")
                .size(sz(11.0))
                .font(medium())
                .color(tok::text_mid()),
        )
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .width(Fill)
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsNetAdd);

        let left = column![
            list_col,
            sp(0, tok::S2),
            add_btn,
        ]
        .spacing(tok::S2)
        .width(Length::Fixed(170.0));

        let form: Element<Message> = if let Some(net) =
            self.settings_draft.networks.get(self.settings_net_idx)
        {
            self.settings_network_form(self.settings_net_idx, net)
        } else {
            text("Add a network to start editing.")
                .size(sz(11.0))
                .color(tok::text_faint())
                .into()
        };

        column![
            settings_section_header("Networks"),
            row![
                container(left).width(Length::Fixed(170.0)),
                container(sp(1, Fill)).style(|_| container::Style {
                    background: Some(Background::Color(tok::border_soft())),
                    ..Default::default()
                }),
                container(form).padding(pad(0.0, 0.0, 0.0, tok::S3)).width(Fill),
            ]
            .spacing(tok::S3),
            sp(0, tok::S2),
            text("Network changes apply on the next restart.")
                .size(sz(10.0))
                .color(tok::text_faint()),
        ]
        .spacing(tok::S3)
        .into()
    }

    fn settings_network_list_row(&self, i: usize, name: &str) -> Element<'_, Message> {
        let selected = i == self.settings_net_idx;
        let label_color = if selected { tok::text() } else { tok::text_muted() };
        button(
            text(truncate(name, 16))
                .size(sz(11.0))
                .font(medium())
                .color(label_color),
        )
        .on_press(Message::SettingsNetSelect(i))
        .width(Fill)
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .style(move |_, status| section_btn_style(selected, status))
        .into()
    }

    fn settings_network_form(
        &self,
        idx: usize,
        n: &NetworkConfig,
    ) -> Element<'_, Message> {
        let mode = match n.auth_mode() {
            config::AuthMode::None => SettingsAuthMode::None,
            config::AuthMode::NickServ => SettingsAuthMode::NickServ,
            config::AuthMode::SaslPlain => SettingsAuthMode::SaslPlain,
            config::AuthMode::SaslExternal => SettingsAuthMode::SaslExternal,
        };

        let radios: Vec<Element<Message>> = vec![
            radio("None", SettingsAuthMode::None, Some(mode), Message::SettingsNetAuthMode)
                .text_size(sz(11.0))
                .size(14.0)
                .into(),
            radio(
                "NickServ",
                SettingsAuthMode::NickServ,
                Some(mode),
                Message::SettingsNetAuthMode,
            )
            .text_size(sz(11.0))
            .size(14.0)
            .into(),
            radio(
                "SASL PLAIN",
                SettingsAuthMode::SaslPlain,
                Some(mode),
                Message::SettingsNetAuthMode,
            )
            .text_size(sz(11.0))
            .size(14.0)
            .into(),
            radio(
                "SASL EXTERNAL",
                SettingsAuthMode::SaslExternal,
                Some(mode),
                Message::SettingsNetAuthMode,
            )
            .text_size(sz(11.0))
            .size(14.0)
            .into(),
        ];
        let auth_row: Element<Message> = row(radios)
            .spacing(tok::S3)
            .align_y(iced::Alignment::Center)
            .into();

        let auth_fields: Element<Message> = match mode {
            SettingsAuthMode::None => sp(0, 0).into(),
            SettingsAuthMode::NickServ => column![settings_row(
                "Nick password",
                settings_password_input(
                    "your NickServ password",
                    n.nick_password.clone().unwrap_or_default(),
                    NetField::NickPassword,
                ),
            )]
            .into(),
            SettingsAuthMode::SaslPlain => column![
                settings_row(
                    "SASL user",
                    settings_text_input(
                        "default: your nickname",
                        n.sasl_username.clone().unwrap_or_default(),
                        NetField::SaslUsername,
                    ),
                ),
                settings_row(
                    "SASL password",
                    settings_password_input(
                        "your SASL password",
                        n.sasl_password.clone().unwrap_or_default(),
                        NetField::SaslPassword,
                    ),
                ),
            ]
            .spacing(tok::S2)
            .into(),
            SettingsAuthMode::SaslExternal => column![
                settings_row(
                    "Client cert (.p12)",
                    settings_text_input(
                        "/absolute/path/to/client.p12",
                        n.client_cert_path.clone().unwrap_or_default(),
                        NetField::ClientCertPath,
                    ),
                ),
                settings_row(
                    "Cert passphrase",
                    settings_password_input(
                        "non-empty on macOS",
                        n.client_cert_pass.clone().unwrap_or_default(),
                        NetField::ClientCertPass,
                    ),
                ),
            ]
            .spacing(tok::S2)
            .into(),
        };

        let tls_box: Element<Message> = checkbox(n.use_tls)
            .label("TLS")
            .on_toggle(Message::SettingsNetTls)
            .text_size(sz(11.0))
            .size(14.0)
            .into();
        let auto_box: Element<Message> = checkbox(n.autoconnect)
            .label("Autoconnect on startup")
            .on_toggle(Message::SettingsNetAutoconnect)
            .text_size(sz(11.0))
            .size(14.0)
            .into();

        let channel_chips: Vec<Element<Message>> = n
            .channels
            .iter()
            .enumerate()
            .map(|(i, ch)| chip(ch, Message::SettingsNetChannelRemove(i)))
            .collect();
        let channel_row: Element<Message> = if channel_chips.is_empty() {
            text("no auto-join channels")
                .size(sz(11.0))
                .color(tok::text_faint())
                .into()
        } else {
            wrap_row(channel_chips, tok::S2 as f32)
        };

        let channel_input = text_input("#channel", &self.settings_net_channel_input)
            .on_input(Message::SettingsNetChannelInput)
            .on_submit(Message::SettingsNetChannelAdd)
            .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
            .size(sz(11.0))
            .width(Length::Fixed(180.0))
            .style(|_, status| input_style(status));
        let channel_add_btn = button(
            text("+")
                .size(sz(14.0))
                .font(medium())
                .color(tok::text_mid()),
        )
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsNetChannelAdd);

        let remove_btn = button(
            text("Remove this network")
                .size(sz(11.0))
                .color(Color { a: 1.0, ..tok::accent() }),
        )
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .style(|_, status| ghost_btn_style(status))
        .on_press(Message::SettingsNetRemove(idx));

        column![
            settings_row(
                "Name",
                settings_text_input("displayed in sidebar", n.name.clone(), NetField::Name),
            ),
            settings_row(
                "Nickname",
                settings_text_input("your IRC nick", n.nickname.clone(), NetField::Nickname),
            ),
            settings_row(
                "Username",
                settings_text_input(
                    "default: same as nick",
                    n.username.clone().unwrap_or_default(),
                    NetField::Username,
                ),
            ),
            settings_row(
                "Realname",
                settings_text_input(
                    "default: same as nick",
                    n.realname.clone().unwrap_or_default(),
                    NetField::Realname,
                ),
            ),
            settings_row(
                "Server",
                settings_text_input("irc.example.org", n.server.clone(), NetField::Server),
            ),
            settings_row(
                "Port",
                settings_text_input("6697", n.port.to_string(), NetField::Port),
            ),
            settings_row("Transport", tls_box),
            sp(0, tok::S1),
            text("Authentication")
                .size(sz(11.0))
                .font(medium())
                .color(tok::text_mid()),
            auth_row,
            auth_fields,
            sp(0, tok::S1),
            text("Auto-join channels")
                .size(sz(11.0))
                .font(medium())
                .color(tok::text_mid()),
            channel_row,
            row![channel_input, channel_add_btn]
                .spacing(tok::S2)
                .align_y(iced::Alignment::Center),
            sp(0, tok::S1),
            auto_box,
            sp(0, tok::S2),
            remove_btn,
        ]
        .spacing(tok::S2)
        .into()
    }

    fn palette_row(&self, i: usize, item: PaletteItem) -> Element<'_, Message> {
        let selected = i == self.palette_cursor;
        let (prefix, label, hint): (&str, String, String) = match item {
            PaletteItem::Channel(idx) => (
                "#",
                self.channels[idx].name.clone(),
                "jump to channel".into(),
            ),
            PaletteItem::Command { name, hint, .. } => {
                ("›", name.to_string(), hint.to_string())
            }
        };

        let content = row![
            text(prefix).size(sz(12.0)).color(tok::text_faint()).font(medium()).width(16),
            text(label).size(sz(13.0)).color(tok::text()).font(medium()),
            sp(Fill, 0),
            text(hint).size(sz(11.0)).color(tok::text_muted()).font(regular()),
        ]
        .spacing(tok::S3)
        .align_y(iced::Alignment::Center);

        button(content)
            .on_press(Message::PaletteActivateIdx(i))
            .width(Fill)
            .padding(pad(tok::S2 as f32, tok::S4, tok::S2 as f32, tok::S4))
            .style(move |_theme, status| palette_row_style(selected, status))
            .into()
    }

    fn network_row(&self, net: &NetworkState) -> Element<'_, Message> {
        let active = self.active == Some(net.id);
        let dot = status_color(net.status);

        let label_color = if active { tok::text_mid() } else { tok::text_faint() };

        let row_content = row![
            container(sp(5, 5)).style(move |_| container::Style {
                background: Some(Background::Color(dot)),
                border: Border { radius: 2.5.into(), ..Default::default() },
                ..Default::default()
            }),
            text(truncate(&net.cfg.name, 14).to_uppercase())
                .size(sz(11.0))
                .font(medium())
                .color(label_color)
                .wrapping(iced::widget::text::Wrapping::None),
            sp(Fill, 0),
        ]
        .spacing(tok::S2)
        .align_y(iced::Alignment::Center);

        let id = net.id;
        let btn = button(row_content)
            .on_press(Message::NetworkSelected(id))
            .width(Fill)
            .padding(pad(tok::S2 as f32, tok::S3 as f32, tok::S1 as f32, tok::S3 as f32))
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(Color::TRANSPARENT)),
                text_color: tok::text(),
                border: Border { radius: 4.0.into(), ..Default::default() },
                shadow: Shadow::default(),
                ..Default::default()
            });

        mouse_area(btn)
            .on_enter(Message::HoverNetwork(Some(id)))
            .on_exit(Message::HoverNetwork(None))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }

    fn sidebar(&self, width: f32, target: f32) -> Element<'_, Message> {
        let network_rows: Vec<Element<Message>> = self
            .networks
            .iter()
            .map(|n| self.network_row(n))
            .collect();

        let networks_section: Element<Message> = if network_rows.is_empty() {
            // No networks defined — show the "not connected" hint.
            let dot = status_color(self.current_status());
            container(
                row![
                    container(sp(5, 5)).style(move |_| container::Style {
                        background: Some(Background::Color(dot)),
                        border: Border { radius: 2.5.into(), ..Default::default() },
                        ..Default::default()
                    }),
                    text("NOT CONNECTED")
                        .size(sz(11.0))
                        .font(medium())
                        .color(tok::text_faint()),
                ]
                .spacing(tok::S2)
                .align_y(iced::Alignment::Center),
            )
            .padding(pad(tok::S3 as f32, tok::S3 as f32, tok::S2 as f32, tok::S3 as f32))
            .into()
        } else {
            column(network_rows)
                .spacing(0)
                .padding(pad(tok::S3 as f32, tok::S1 as f32, tok::S1 as f32, tok::S1 as f32))
                .into()
        };

        let active_id = self.active;
        let items: Vec<Element<Message>> = self
            .channels
            .iter()
            .enumerate()
            .filter(|(_, ch)| Some(ch.network_id) == active_id)
            .map(|(i, ch)| self.channel_row(i, ch))
            .collect();

        let list = scrollable(
            column(items)
                .spacing(0)
                .padding(pad(tok::S1 as f32, tok::S1 as f32, tok::S2 as f32, tok::S1 as f32)),
        )
        .height(Fill);

        container(
            container(column![networks_section, list].spacing(0))
                .width(Length::Fixed(target))
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(tok::bg_0())),
                    ..Default::default()
                }),
        )
        .width(Length::Fixed(width))
        .height(Fill)
        .clip(true)
        .into()
    }

    fn channel_row(&self, i: usize, ch: &Channel) -> Element<'_, Message> {
        let selected = i == self.selected;
        let (prefix, label) = channel_parts(&ch.name);

        let now = self.now;
        let hover_v = ch.hover_anim.interpolate(0.0f32, 1.0f32, now);
        let select_v = ch.select_anim.interpolate(0.0f32, 1.0f32, now);

        let mention = !selected && ch.has_mention;
        let unread = !selected && ch.has_unread;

        let base_label = if mention {
            tok::text()
        } else {
            tok::text_mid()
        };
        let label_color = blend(base_label, tok::text(), hover_v.max(select_v));
        let prefix_color = blend(tok::text_faint(), tok::text_muted(), hover_v.max(select_v));
        let row_bg = blend(
            blend(Color::TRANSPARENT, tok::bg_hover(), hover_v),
            tok::accent_soft(),
            select_v,
        );

        let dot_color = if mention {
            tok::accent()
        } else if unread {
            Color { a: 0.45, ..tok::text_mid() }
        } else {
            Color::TRANSPARENT
        };
        let dot = container(sp(5, 5)).style(move |_| container::Style {
            background: Some(Background::Color(dot_color)),
            border: Border { radius: 2.5.into(), ..Default::default() },
            ..Default::default()
        });
        let bold_label = selected || mention;

        let row_content = row![
            container(dot).width(Length::Fixed(7.0)).align_x(iced::alignment::Horizontal::Center),
            text(prefix)
                .size(sz(13.0))
                .font(regular())
                .color(prefix_color)
                .width(Length::Fixed(12.0)),
            text(truncate(&label, 18))
                .size(sz(13.0))
                .font(if bold_label { medium() } else { regular() })
                .wrapping(iced::widget::text::Wrapping::None)
                .color(label_color),
        ]
        .spacing(tok::S1)
        .align_y(iced::Alignment::Center);

        let btn = button(row_content)
            .on_press(Message::ChannelSelected(i))
            .width(Fill)
            .padding(pad(tok::S1 as f32, tok::S3 as f32, tok::S1 as f32, tok::S3 as f32))
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(row_bg)),
                text_color: tok::text(),
                border: Border { radius: 4.0.into(), ..Default::default() },
                shadow: Shadow::default(),
                ..Default::default()
            });

        let closable = !ch.name.starts_with('&');
        let close_alpha = hover_v.max(select_v);
        let row_el: Element<Message> = if closable && close_alpha > 0.02 {
            let close = button(
                text("×")
                    .size(sz(15.0))
                    .font(medium())
                    .color(Color { a: close_alpha * 0.8, ..tok::text_mid() }),
            )
            .on_press(Message::CloseChannel(i))
            .padding(pad(0.0, 6.0, 2.0, 6.0))
            .style(move |_theme, status| close_btn_style(status, close_alpha));
            let close = mouse_area(close)
                .interaction(iced::mouse::Interaction::Pointer);
            let overlay = container(close)
                .width(Fill)
                .height(Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Center)
                .padding(pad(0.0, tok::S2 as f32, 0.0, 0.0));
            stack![btn, overlay].into()
        } else {
            btn.into()
        };

        mouse_area(row_el)
            .on_enter(Message::HoverChannel(Some(i)))
            .on_exit(Message::HoverChannel(None))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }

    fn chat_pane(&self) -> Element<'_, Message> {
        let ch = &self.channels[self.selected];

        let (prefix, label) = channel_parts(&ch.name);

        let net_name = self
            .net(ch.network_id)
            .map(|n| n.cfg.name.clone())
            .unwrap_or_default();
        let members_count = ch.members.len();

        let mut meta_parts: Vec<String> = Vec::new();
        if !net_name.is_empty() {
            meta_parts.push(format!("@ {}", net_name));
        }
        if members_count > 0 {
            meta_parts.push(format!(
                "{} {}",
                members_count,
                if members_count == 1 { "user" } else { "users" }
            ));
        }
        let meta_el: Element<Message> = if meta_parts.is_empty() {
            sp(0, 0).into()
        } else {
            text(meta_parts.join(" — "))
                .size(sz(12.0))
                .color(tok::text_muted())
                .font(regular())
                .into()
        };

        let header_title: Element<Message> = row![
            text(prefix).size(sz(13.0)).font(regular()).color(tok::text_muted()),
            text(label).size(sz(13.0)).font(medium()).color(tok::text()),
            sp(tok::S3, 0),
            meta_el,
        ]
        .spacing(tok::S1)
        .align_y(iced::Alignment::Center)
        .into();
        let header_topic: Element<Message> = match &ch.topic {
            Some(t) => text(t.clone()).size(sz(11.0)).color(tok::text_muted()).into(),
            None => sp(0, 0).into(),
        };

        let sidebar_open = self.sidebar_anim.value();
        let members_open = self.members_anim.value();

        let left_toggle = toggle_button(
            if sidebar_open { "‹" } else { "›" },
            Message::ToggleSidebar,
        );
        let right_toggle = toggle_button(
            if members_open { "›" } else { "‹" },
            Message::ToggleMembers,
        );

        let header = container(
            row![
                left_toggle,
                column![header_title, header_topic].spacing(tok::S1),
                sp(Fill, 0),
                right_toggle,
            ]
            .spacing(tok::S3)
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S2, tok::S3, tok::S2, tok::S3))
        .width(Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(tok::bg_1())),
            border: Border {
                color: tok::border_soft(),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });

        let msgs = self.render_messages(ch);

        let msg_area = scrollable(
            column(msgs)
                .spacing(0)
                .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
                .width(Fill),
        )
        .height(Fill)
        .width(Fill)
        .anchor_bottom();

        let msg_area: Element<Message> = if self.search.is_some() {
            stack![msg_area, self.search_overlay()].into()
        } else {
            msg_area.into()
        };

        let placeholder = compose_placeholder(&ch.name);
        let has_text = !self.input.trim().is_empty();

        let text_field = text_input(&placeholder, &self.input)
            .id(COMPOSE_INPUT_ID)
            .on_input(Message::InputChanged)
            .on_submit(Message::SendMessage)
            .size(sz(13.0))
            .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
            .style(|_theme, status| input_style(status));

        let send_btn = button(
            container(text("↑").size(sz(16.0)).font(medium()).color(if has_text {
                Color::WHITE
            } else {
                tok::text_faint()
            }))
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center),
        )
        .on_press_maybe(has_text.then_some(Message::SendMessage))
        .padding(0)
        .style(move |_theme, status| send_button_style(has_text, status));

        let send_btn: Element<Message> = if has_text {
            mouse_area(send_btn)
                .interaction(iced::mouse::Interaction::Pointer)
                .into()
        } else {
            send_btn.into()
        };

        let emoji_btn = mouse_area(
            button(
                container(text("☺").size(sz(16.0)).color(tok::text_muted()).font(regular()))
                    .width(Length::Fixed(36.0))
                    .height(Length::Fixed(36.0))
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center),
            )
            .on_press(Message::EmojiPickerToggle)
            .padding(0)
            .style(|_theme, status| ghost_button_style(status)),
        )
        .interaction(iced::mouse::Interaction::Pointer);

        // Attach button only when there's a usable upload target (custom
        // uploader configured, or the network advertises FILEHOST).
        let can_attach = self.has_upload_target();
        let attach_btn: Element<Message> = if can_attach {
            let glyph = if self.uploading { "⋯" } else { "📎" };
            let press = (!self.uploading).then_some(Message::AttachFilePressed);
            mouse_area(
                button(
                    container(text(glyph).size(sz(15.0)).color(tok::text_muted()).font(regular()))
                        .width(Length::Fixed(36.0))
                        .height(Length::Fixed(36.0))
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center),
                )
                .on_press_maybe(press)
                .padding(0)
                .style(|_theme, status| ghost_button_style(status)),
            )
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
        } else {
            sp(0, 0).into()
        };

        let dropping = self.file_hover && can_attach && !self.uploading;
        let input = container(
            row![text_field, attach_btn, emoji_btn, send_btn]
                .spacing(tok::S2)
                .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S2, tok::S4, tok::S3, tok::S4))
        .style(move |_| container::Style {
            border: Border {
                color: if dropping { tok::accent() } else { Color::TRANSPARENT },
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });

        let typing_bar: Element<Message> = match self.typing_text() {
            Some(body) => container(
                text(body)
                    .size(sz(11.0))
                    .color(tok::text_faint())
                    .font(regular()),
            )
            .padding(pad(0.0, tok::S4 as f32, tok::S1 as f32, tok::S4 as f32))
            .width(Fill)
            .into(),
            None => sp(0, 0).into(),
        };

        container(column![header, msg_area, typing_bar, input])
            .width(Fill)
            .height(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_1())),
                ..Default::default()
            })
            .into()
    }

    fn render_messages<'a>(&'a self, ch: &'a Channel) -> Vec<Element<'a, Message>> {
        // Mentions are scoped to the channel's network, not the global active.
        let my_nick = self
            .net(ch.network_id)
            .map(|n| n.cfg.nickname.as_str())
            .unwrap_or("");

        let baseline = ch.fade_baseline;

        let mut out: Vec<Element<Message>> = Vec::with_capacity(ch.messages.len() * 2);
        let mut prev_day: Option<&str> = None;
        let mut prev_nick: Option<&str> = None;
        let mut prev_secs: u64 = 0;
        let marker_at = ch
            .read_marker_idx
            .filter(|&i| i < ch.messages.len());
        let search_q = self.search_query_lower();
        let search_q_ref = search_q.as_deref();

        for (i, m) in ch.messages.iter().enumerate() {
            if prev_day != Some(m.day.as_str()) {
                out.push(self.day_separator(&m.day));
                prev_day = Some(m.day.as_str());
                prev_nick = None;
            }
            if marker_at == Some(i) {
                out.push(self.read_marker_row());
                prev_nick = None;
            }

            if m.kind == MsgKind::JoinPart {
                if ch.hide_joinpart {
                    continue;
                }
                out.push(self.joinpart_row(m, baseline));
                prev_nick = None;
                continue;
            }

            let grouped = matches!(m.kind, MsgKind::Chat | MsgKind::System)
                && prev_nick == Some(m.nick.as_str())
                && m.mono_secs.saturating_sub(prev_secs) < GROUP_SECS;

            let nick_dimmed = self.dimmed_nicks.contains(&m.nick);
            let focus_dimmed = ch.dimm
                && matches!(m.kind, MsgKind::Chat | MsgKind::Action)
                && !mentions(&m.body, my_nick)
                && m.nick != my_nick;

            let dim_level = if nick_dimmed {
                0.15
            } else if focus_dimmed {
                0.3
            } else {
                1.0
            };

            let line = self.message_line(m, grouped, dim_level, baseline, search_q_ref);
            let line_el: Element<Message> = if let Some(msgid) = m.msgid.clone() {
                let channel_idx = self.selected;
                mouse_area(line)
                    .on_right_press(Message::MessageContextOpen {
                        channel_idx,
                        msgid,
                    })
                    .into()
            } else {
                line
            };
            out.push(line_el);

            if let Some(ctx) = self.message_context.as_ref() {
                if ctx.channel_idx == self.selected
                    && m.msgid.as_deref() == Some(ctx.msgid.as_str())
                {
                    out.push(message_action_bar(ctx));
                }
            }

            if !m.reactions.is_empty() {
                out.push(reactions_row(&m.reactions));
            }

            prev_nick = Some(m.nick.as_str());
            prev_secs = m.mono_secs;
        }
        out
    }

    fn joinpart_row<'a>(&'a self, m: &'a ChatMessage, baseline: Instant) -> Element<'a, Message> {
        let start = m.inserted_at.max(baseline);
        let age_ms = start.elapsed().as_millis().min(FADE_MS);
        let t = age_ms as f32 / FADE_MS as f32;
        let alpha = (1.0 - (1.0 - t).powi(3)) * 0.35;

        container(
            row![
                sp(TIME_W, 0),
                sp(NICK_W, 0),
                text(m.body.clone())
                    .size(sz(11.0))
                    .color(Color { a: alpha, ..tok::text_muted() })
                    .font(regular()),
            ]
            .spacing(tok::S3)
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(0.0, 0.0, 0.0, 0.0))
        .width(Fill)
        .into()
    }

    fn search_overlay(&self) -> Element<'_, Message> {
        let s = self.search.as_ref().expect("search_overlay called when closed");
        let count = self.search_match_count();
        let count_label: Element<Message> = if s.query.trim().is_empty() {
            sp(0, 0).into()
        } else {
            text(format!("{count}"))
                .size(sz(11.0))
                .color(if count == 0 { tok::text_faint() } else { tok::text_muted() })
                .font(regular())
                .into()
        };

        let input = text_input("search this channel…", &s.query)
            .id(SEARCH_INPUT_ID)
            .on_input(Message::SearchQuery)
            .padding(pad(tok::S2, tok::S3, tok::S2, tok::S3))
            .size(sz(12.0))
            .width(Length::Fixed(240.0))
            .style(|_theme, status| input_style(status));

        let close_btn = mouse_area(
            button(
                container(text("×").size(sz(14.0)).color(tok::text_muted()).font(regular()))
                    .width(Length::Fixed(28.0))
                    .height(Length::Fixed(28.0))
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center),
            )
            .on_press(Message::SearchClose)
            .padding(0)
            .style(|_theme, status| ghost_button_style(status)),
        )
        .interaction(iced::mouse::Interaction::Pointer);

        let bar = container(
            row![input, count_label, close_btn]
                .spacing(tok::S3)
                .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S2, tok::S3, tok::S2, tok::S3))
        .style(|_| container::Style {
            background: Some(Background::Color(tok::bg_1())),
            border: Border {
                color: tok::border(),
                width: 1.0,
                radius: 10.0.into(),
            },
            shadow: Shadow {
                color: Color { a: 0.35, ..Color::BLACK },
                offset: iced::Vector::new(0.0, 6.0),
                blur_radius: 24.0,
            },
            ..Default::default()
        });

        container(bar)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .padding(pad(tok::S3 as f32, tok::S4 as f32, 0.0, 0.0))
            .into()
    }

    fn read_marker_row<'a>(&'a self) -> Element<'a, Message> {
        let accent = tok::accent();
        let faint_accent = Color { a: 0.45, ..accent };
        container(
            row![
                container(sp(Fill, 1)).style(move |_| container::Style {
                    background: Some(Background::Color(faint_accent)),
                    ..Default::default()
                }),
                text("new").size(sz(10.0)).color(accent).font(medium()),
                container(sp(Fill, 1)).style(move |_| container::Style {
                    background: Some(Background::Color(faint_accent)),
                    ..Default::default()
                }),
            ]
            .spacing(tok::S3)
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S3 as f32, 0.0, tok::S2 as f32, 0.0))
        .width(Fill)
        .into()
    }

    fn day_separator<'a>(&'a self, day: &str) -> Element<'a, Message> {
        let label = day.to_string();
        container(
            row![
                container(sp(Fill, 1)).style(|_| container::Style {
                    background: Some(Background::Color(tok::border_soft())),
                    ..Default::default()
                }),
                text(label).size(sz(10.0)).color(tok::text_faint()).font(medium()),
                container(sp(Fill, 1)).style(|_| container::Style {
                    background: Some(Background::Color(tok::border_soft())),
                    ..Default::default()
                }),
            ]
            .spacing(tok::S3)
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S4 as f32, 0.0, tok::S3 as f32, 0.0))
        .width(Fill)
        .into()
    }

    fn message_line<'a>(
        &'a self,
        m: &'a ChatMessage,
        grouped: bool,
        dim_level: f32,
        baseline: Instant,
        search_q: Option<&str>,
    ) -> Element<'a, Message> {
        let start = m.inserted_at.max(baseline);
        let age_ms = start.elapsed().as_millis().min(FADE_MS);
        let t = age_ms as f32 / FADE_MS as f32;
        let fade = 1.0 - (1.0 - t).powi(3); // ease-out cubic
        let alpha = fade * dim_level;

        let n_color = nick_color(&m.nick);

        // Two-column layout: fixed-width timestamp on the left, rich_text
        // with nick + body on the right. iced wraps the rich_text as a
        // paragraph within its column, so wrapped lines fall to the
        // rich_text's left edge — i.e. directly under the nick. The
        // timestamp column has fixed width, so wraps line up across all
        // messages. Grouped continuations show the timestamp for vertical
        // alignment but skip the nick span, so their body starts at the
        // same X as the nick of the head message in the group.
        let time_color = Color { a: 0.7 * alpha, ..tok::text_faint() };
        let nick_short = truncate(&m.nick, 12).to_string();

        let (body_font, body_color) = if m.kind == MsgKind::Action {
            (italic(), tok::text_mid())
        } else {
            (regular(), tok::text())
        };
        let url_color = Color { a: alpha, ..tok::accent() };
        let text_color = Color { a: alpha, ..body_color };

        let my_nick = self
            .active_net()
            .map(|n| n.cfg.nickname.as_str())
            .unwrap_or("");
        let nick_clickable = !grouped
            && m.kind != MsgKind::System
            && !m.nick.is_empty()
            && m.nick != my_nick;

        let time_el: Element<Message> = text(m.time.clone())
            .size(sz(11.0))
            .color(time_color)
            .font(regular())
            .width(TIME_W)
            .into();

        let mut spans: Vec<iced::widget::text::Span<String>> = Vec::new();

        if !grouped {
            let mut nick_span = iced::widget::span(format!("{} ", nick_short))
                .color(Color { a: alpha, ..n_color })
                .font(medium());
            if nick_clickable {
                // dm:nick links are intercepted in OpenUrl so the OS opener
                // never sees them.
                nick_span = nick_span.link(format!("dm:{}", m.nick));
            }
            spans.push(nick_span);
        }

        let hl_bg = Color { a: 0.55 * alpha, ..tok::accent() };
        for seg in body_segments(&m.body) {
            match seg {
                BodySeg::Text(t) => {
                    let parts = match search_q {
                        Some(q) => split_on_query(t, q),
                        None => vec![(t, false)],
                    };
                    for (chunk, is_match) in parts {
                        if chunk.is_empty() {
                            continue;
                        }
                        let mut sp = iced::widget::span(chunk.to_string())
                            .color(text_color)
                            .font(body_font);
                        if is_match {
                            sp = sp.background(hl_bg);
                        }
                        spans.push(sp);
                    }
                }
                BodySeg::Url(u) => {
                    let parts = match search_q {
                        Some(q) => split_on_query(u, q),
                        None => vec![(u, false)],
                    };
                    for (chunk, is_match) in parts {
                        if chunk.is_empty() {
                            continue;
                        }
                        let mut sp = iced::widget::span(chunk.to_string())
                            .color(url_color)
                            .font(body_font)
                            .underline(true)
                            .link(u.to_string());
                        if is_match {
                            sp = sp.background(hl_bg);
                        }
                        spans.push(sp);
                    }
                }
            }
        }

        let body_rich = iced::widget::rich_text(spans)
            .size(sz(13.0))
            .width(Length::Fill)
            .on_link_click(Message::OpenUrl);

        let top_pad = if grouped { 0.0 } else { tok::S1 as f32 };

        let media_els: Vec<Element<Message>> = extract_urls(&m.body)
            .iter()
            .filter_map(|url| match self.media_cache.get(url) {
                Some(MediaState::Image { handle, w, h }) => {
                    Some(image_preview(url, handle.clone(), *w, *h, alpha))
                }
                Some(MediaState::File { kind, content_type, size }) => {
                    Some(file_card(url, *kind, content_type, *size, alpha))
                }
                Some(MediaState::LinkCard { title, description, host, image }) => {
                    Some(link_card(url, title.as_deref(), description.as_deref(), host, image, alpha))
                }
                Some(MediaState::Error(e)) => Some(media_error(url, e, alpha)),
                _ => None,
            })
            .collect();

        let body_col: Element<Message> = if media_els.is_empty() {
            body_rich.into()
        } else {
            let mut col = column![body_rich].spacing(tok::S2);
            for el in media_els {
                col = col.push(el);
            }
            col.width(Length::Fill).into()
        };

        container(
            row![time_el, body_col]
                .spacing(tok::S3)
                .align_y(iced::Alignment::Start),
        )
        .padding(pad(top_pad, 0.0, 0.0, 0.0))
        .width(Fill)
        .into()
    }

    fn member_pane(&self, width: f32, target: f32) -> Element<'_, Message> {
        let ch = &self.channels[self.selected];

        let ctx_nick = self.member_context.as_ref().and_then(|c| {
            if c.channel_idx == self.selected { Some(c.nick.as_str()) } else { None }
        });
        let mut items: Vec<Element<Message>> = Vec::with_capacity(ch.members.len() + 1);
        for (i, m) in ch.members.iter().enumerate() {
            items.push(self.member_row(i, m));
            if ctx_nick.is_some_and(|n| n == m.as_str()) {
                items.push(self.member_action_bar(m));
            }
        }

        let list = scrollable(
            column(items)
                .spacing(0)
                .padding(pad(tok::S2 as f32, 0.0, tok::S2 as f32, 0.0)),
        )
        .height(Fill);

        container(
            container(list)
                .width(Length::Fixed(target))
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(tok::bg_0())),
                    ..Default::default()
                }),
        )
        .width(Length::Fixed(width))
        .height(Fill)
        .clip(true)
        .into()
    }

    fn member_row(&self, i: usize, nick: &str) -> Element<'_, Message> {
        let hovered = self.hovered_member == Some(i);
        let nick_owned = nick.to_string();
        let away = self.is_nick_away(nick);
        let prefix = self.member_prefix(nick);

        // Away users: fade both the colored dot and the nick text.
        // Prefix takes precedence on the dot color (ops greenish, voice cyan).
        let dot_color = if away {
            Color::from_rgb(0.55, 0.6, 0.6)
        } else {
            match prefix {
                Some('~') | Some('&') | Some('@') => Color::from_rgb(0.4, 0.8, 0.55),
                Some('%') => Color::from_rgb(0.5, 0.7, 0.85),
                Some('+') => Color::from_rgb(0.55, 0.75, 0.95),
                _ => Color::from_rgb(0.55, 0.6, 0.6),
            }
        };
        let mut text_color = nick_color(nick);
        if away {
            text_color.a *= 0.45;
        }

        // Render the highest-priority prefix as a leading glyph.
        let label = match prefix {
            Some(p) => format!("{p}{}", truncate(nick, 14)),
            None => truncate(nick, 14).to_string(),
        };

        let row_content = row![
            container(sp(6, 6)).style(move |_| container::Style {
                background: Some(Background::Color(dot_color)),
                border: Border { radius: 3.0.into(), ..Default::default() },
                ..Default::default()
            }),
            text(label)
                .size(sz(12.0))
                .line_height(iced::widget::text::LineHeight::Absolute(iced::Pixels(sz(14.0))))
                .color(text_color)
                .font(if hovered { medium() } else { regular() })
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(tok::S2)
        .align_y(iced::Alignment::Center);

        let btn = button(row_content)
            .on_press(Message::StartDmWith(nick_owned))
            .width(Fill)
            .padding(pad(1.0, tok::S4 as f32, 1.0, tok::S4 as f32))
            .style(move |_theme, status| member_row_style(status));

        mouse_area(btn)
            .on_enter(Message::HoverMember(Some(i)))
            .on_exit(Message::HoverMember(None))
            .on_right_press(Message::MemberContextOpen { nick: nick.to_string() })
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }

    fn member_action_bar(&self, nick: &str) -> Element<'_, Message> {
        let prefix = self.member_prefix(nick);
        let is_op = matches!(prefix, Some('~') | Some('&') | Some('@'));
        let is_voiced = matches!(prefix, Some('+'));
        let ignored = self.is_ignored(nick);

        let mut items: Vec<Element<Message>> = Vec::new();
        let header = container(
            text(format!("@{}", nick))
                .size(sz(10.5))
                .color(tok::text_faint())
                .font(regular()),
        )
        .padding(pad(2.0, 8.0, 2.0, 8.0));
        items.push(header.into());

        items.push(member_action_button("Message".into(), Message::MemberContextDm));
        items.push(member_action_button("Whois".into(), Message::MemberContextWhois));
        items.push(member_action_button(
            if ignored { "Unignore".into() } else { "Ignore".into() },
            Message::MemberContextIgnoreToggle,
        ));
        items.push(member_action_button(
            if is_op { "Deop".into() } else { "Op".into() },
            Message::MemberContextOpToggle,
        ));
        items.push(member_action_button(
            if is_voiced { "Devoice".into() } else { "Voice".into() },
            Message::MemberContextVoiceToggle,
        ));
        items.push(member_action_button("Kick".into(), Message::MemberContextKick));
        items.push(member_action_button("Ban".into(), Message::MemberContextBan));
        items.push(member_action_button("Dismiss".into(), Message::MemberContextClose));

        container(column(items).spacing(1))
            .padding(pad(2.0, tok::S2 as f32, tok::S2 as f32, tok::S2 as f32))
            .width(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_1())),
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: tok::border_soft(),
                },
                ..Default::default()
            })
            .into()
    }

    fn theme(&self) -> Theme {
        if theme::current().is_dark { Theme::Dark } else { Theme::Light }
    }
}

fn toggle_button<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(
        container(text(label).size(sz(14.0)).color(tok::text_mid()).font(medium()))
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0))
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center),
    )
    .on_press(msg)
    .padding(0)
    .style(|_theme, status| button::Style {
        background: Some(Background::Color(match status {
            button::Status::Hovered => tok::bg_hover(),
            _ => Color::TRANSPARENT,
        })),
        text_color: tok::text_mid(),
        border: Border { radius: 5.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    })
    .into()
}

fn input_style(status: iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    use iced::widget::text_input::Status;
    let (border_color, border_width) = match status {
        Status::Focused { .. } => (tok::accent_ring(), 1.5),
        Status::Hovered => (tok::border(), 1.0),
        _ => (tok::border(), 1.0),
    };
    iced::widget::text_input::Style {
        background: Background::Color(tok::bg_2()),
        border: Border {
            color: border_color,
            width: border_width,
            radius: 8.0.into(),
        },
        icon: tok::text_muted(),
        placeholder: tok::text_faint(),
        value: tok::text(),
        selection: tok::accent_soft(),
    }
}

fn palette_input_style(
    _theme: &Theme,
    status: iced::widget::text_input::Status,
) -> iced::widget::text_input::Style {
    use iced::widget::text_input::Status;
    let border_color = match status {
        Status::Focused { .. } => tok::accent_ring(),
        _ => Color::TRANSPARENT,
    };
    iced::widget::text_input::Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            color: border_color,
            width: 0.0,
            radius: 0.0.into(),
        },
        icon: tok::text_muted(),
        placeholder: tok::text_faint(),
        value: tok::text(),
        selection: tok::accent_soft(),
    }
}

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_PREVIEW_W: f32 = 280.0;
const MAX_PREVIEW_H: f32 = 200.0;

fn open_url(url: &str) {
    use std::process::Command;
    #[cfg(target_os = "macos")]
    let cmd = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let cmd = Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let cmd = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    let _ = cmd;
}

#[derive(Debug)]
enum BodySeg<'a> {
    Text(&'a str),
    Url(&'a str),
}

/// Splits a string into segments, marking spans that case-insensitively
/// match `q_lower` (which must already be ASCII-lowercased). ASCII case
/// folding only — Unicode case mismatches won't match.
fn split_on_query<'a>(s: &'a str, q_lower: &str) -> Vec<(&'a str, bool)> {
    if q_lower.is_empty() || s.is_empty() {
        return vec![(s, false)];
    }
    let hay = s.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < s.len() {
        match hay[cursor..].find(q_lower) {
            Some(pos) => {
                let abs = cursor + pos;
                if abs > cursor {
                    out.push((&s[cursor..abs], false));
                }
                let end = abs + q_lower.len();
                out.push((&s[abs..end], true));
                cursor = end;
            }
            None => {
                out.push((&s[cursor..], false));
                break;
            }
        }
    }
    out
}

/// Splits a chat body into alternating text / URL segments. URL detection
/// matches the same `http(s)://` prefix rule as `extract_urls` and trims
/// trailing punctuation so a sentence ending with a link reads naturally.
fn body_segments(body: &str) -> Vec<BodySeg<'_>> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while cursor < body.len() {
        let rest = &body[cursor..];
        let next_https = rest.find("https://");
        let next_http = rest.find("http://");
        let next_off = match (next_https, next_http) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let abs = match next_off {
            Some(o) => cursor + o,
            None => {
                if cursor < body.len() {
                    out.push(BodySeg::Text(&body[cursor..]));
                }
                break;
            }
        };
        if abs > cursor {
            out.push(BodySeg::Text(&body[cursor..abs]));
        }
        let after = &body[abs..];
        let token_end = after.find(char::is_whitespace).unwrap_or(after.len());
        let token = &after[..token_end];
        // Strip trailing punctuation so it stays as text (the URL itself
        // shouldn't include a sentence-ending period or comma).
        let trimmed_end = token
            .trim_end_matches(|c: char| matches!(c, ')' | ']' | '"' | '\'' | ',' | '.' | ';' | ':' | '!' | '?'))
            .len();
        out.push(BodySeg::Url(&token[..trimmed_end]));
        if trimmed_end < token.len() {
            out.push(BodySeg::Text(&token[trimmed_end..]));
        }
        cursor = abs + token.len();
    }
    out
}

fn extract_urls(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in body.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            matches!(c, '<' | '>' | '(' | ')' | '[' | ']' | '"' | '\'' | ',' | '.' | ';' | ':' | '!' | '?')
        });
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            if !out.iter().any(|u: &String| u == trimmed) {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

const MAX_HTML_BYTES: usize = 256 * 1024;

// Strip characters that would break the Content-Disposition header (quotes,
// backslashes, control chars / CRLF header injection).
fn header_safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    if cleaned.is_empty() { "file".into() } else { cleaned }
}

// A resolved upload destination for one file.
enum UploadJob {
    // IRC server's advertised FILEHOST endpoint.
    Filehost {
        endpoint: String,
        auth: Option<(String, String)>,
    },
    // User-configured custom HTTP uploader (e.g. a pastebin).
    Custom {
        url: String,
        token: Option<String>,
        field: String,
        response_kind: String,
        response_key: String,
    },
}

async fn run_upload(job: UploadJob, path: std::path::PathBuf) -> Result<String, String> {
    match job {
        UploadJob::Filehost { endpoint, auth } => upload_file(endpoint, auth, path).await,
        UploadJob::Custom {
            url,
            token,
            field,
            response_kind,
            response_key,
        } => upload_custom(url, token, field, response_kind, response_key, path).await,
    }
}

// Upload to a custom HTTP endpoint. Sends the file either as a multipart form
// field (when `field` is non-empty) or as a raw request body, optionally with
// a Bearer token, then extracts the resulting URL from the response per
// `response_kind` ("json" + `response_key`, "location" header, or "text").
async fn upload_custom(
    url: String,
    token: Option<String>,
    field: String,
    response_kind: String,
    response_key: String,
    path: std::path::PathBuf,
) -> Result<String, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("read file: {e}"))?;
    if bytes.is_empty() {
        return Err("file is empty".into());
    }
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(header_safe_filename)
        .unwrap_or_else(|| "file".into());
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent("murmur/0.2")
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.post(&url);
    if let Some(t) = token.as_ref().filter(|t| !t.is_empty()) {
        req = req.bearer_auth(t);
    }
    if field.trim().is_empty() {
        req = req
            .header(reqwest::header::CONTENT_TYPE, &mime)
            .header(
                reqwest::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            )
            .body(bytes);
    } else {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(&mime)
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part(field.trim().to_string(), part);
        req = req.multipart(form);
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect();
        return Err(if snippet.is_empty() {
            format!("HTTP {}", status.as_u16())
        } else {
            format!("HTTP {} — {snippet}", status.as_u16())
        });
    }

    match response_kind.trim() {
        "location" => {
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("response had no Location header")?;
            Ok(resolve_url(&url, loc))
        }
        "text" => {
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let u = body.trim();
            if u.is_empty() {
                Err("empty response body".into())
            } else {
                Ok(resolve_url(&url, u))
            }
        }
        _ => {
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let v: serde_json::Value =
                serde_json::from_str(&body).map_err(|e| format!("parse json: {e}"))?;
            let key = if response_key.trim().is_empty() {
                "url"
            } else {
                response_key.trim()
            };
            let found = v
                .get(key)
                .and_then(|x| x.as_str())
                .ok_or_else(|| format!("key {key:?} not found in JSON response"))?;
            Ok(resolve_url(&url, found))
        }
    }
}

// IRCv3 FILEHOST (soju.im/FILEHOST) upload: POST the raw file bytes to the
// advertised endpoint with the same credentials used on the IRC connection,
// then read the uploaded file's URL from the 201 `Location` header.
async fn upload_file(
    endpoint: String,
    auth: Option<(String, String)>,
    path: std::path::PathBuf,
) -> Result<String, String> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("read file: {e}"))?;
    if bytes.is_empty() {
        return Err("file is empty".into());
    }
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(header_safe_filename)
        .unwrap_or_else(|| "file".into());
    let mime = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent("murmur/0.2")
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client
        .post(&endpoint)
        .header(reqwest::header::CONTENT_TYPE, &mime)
        .header(
            reqwest::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(bytes);
    if let Some((user, pass)) = auth {
        req = req.basic_auth(user, Some(pass));
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if status.as_u16() != 201 {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = snippet.chars().take(120).collect();
        return Err(if snippet.is_empty() {
            format!("HTTP {}", status.as_u16())
        } else {
            format!("HTTP {} — {snippet}", status.as_u16())
        });
    }
    let location = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or("server returned 201 without a Location header")?;
    Ok(resolve_url(&endpoint, location))
}

async fn fetch_media(url: String) -> FetchedMedia {
    let make_err = |e: String| FetchedMedia { url: url.clone(), state: MediaState::Error(e) };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .user_agent("murmur/0.1")
        .build()
    {
        Ok(c) => c,
        Err(e) => return make_err(e.to_string()),
    };

    let head = match client.head(&url).send().await {
        Ok(r) => r,
        Err(e) => return make_err(e.to_string()),
    };
    if !head.status().is_success() {
        return make_err(format!("HTTP {}", head.status().as_u16()));
    }

    let ct = head
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let size = head
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    if ct.starts_with("image/") {
        if let Some(sz) = size {
            if sz > MAX_IMAGE_BYTES {
                return make_err(format!("image too large ({})", human_size(sz)));
            }
        }
        match fetch_image(&client, &url).await {
            Ok((handle, w, h)) => {
                return FetchedMedia { url, state: MediaState::Image { handle, w, h } };
            }
            Err(e) => return make_err(e),
        }
    }

    if ct.starts_with("text/html") {
        // Pull the page, parse OpenGraph / twitter / <title>, then build a
        // link card. If og:image is present, fetch it as the card's preview.
        let html = match fetch_html_text(&client, &url).await {
            Ok(s) => s,
            Err(_) => return FetchedMedia { url, state: MediaState::Skipped },
        };
        let meta = parse_html_meta(&html);
        let has_og_or_twitter = meta
            .keys()
            .any(|k| k.starts_with("og:") || k.starts_with("twitter:"));
        let has_title = meta
            .get("title")
            .map(|s| !collapse_ws(s).is_empty())
            .unwrap_or(false);

        // Image-wrapper pages (paste sites etc.) embed an <img> but ship
        // no OpenGraph/Twitter metadata and no meaningful <title>. Promote
        // those directly to a full image preview instead of a sparse link
        // card. Real pages with a <title> (wikis, blogs without og:) get
        // a link card instead, so we don't hijack the preview with their
        // logo or footer image.
        //
        // Exception: a paste/file viewer whose <title> is literally an image
        // filename (e.g. "Screenshot ….png") is an image page even though it
        // has a title — follow its embedded <img> too. The extension check
        // keeps real articles ("Breaking News") as link cards.
        // Match an image extension anywhere in the title so a site-name suffix
        // (e.g. "shot.png — pastebin") still counts. Followed by end or a
        // non-alphanumeric so "report.pngx" or prose like "the apng format"
        // don't trip it.
        let title_is_image_name = meta
            .get("title")
            .map(|t| {
                let t = t.to_ascii_lowercase();
                [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".heic", ".heif", ".avif", ".svg"]
                    .iter()
                    .any(|ext| match t.find(ext) {
                        Some(i) => t[i + ext.len()..]
                            .chars()
                            .next()
                            .map_or(true, |c| !c.is_alphanumeric()),
                        None => false,
                    })
            })
            .unwrap_or(false);
        if (!has_og_or_twitter && !has_title) || title_is_image_name {
            if let Some(img_src) = extract_first_img_src(&html) {
                let img_url = resolve_url(&url, &img_src);
                if let Ok((handle, w, h)) = fetch_image(&client, &img_url).await {
                    return FetchedMedia { url, state: MediaState::Image { handle, w, h } };
                }
            }
        }

        let title = meta
            .get("og:title")
            .or_else(|| meta.get("twitter:title"))
            .or_else(|| meta.get("title"))
            .map(|s| collapse_ws(s));
        let description = meta
            .get("og:description")
            .or_else(|| meta.get("twitter:description"))
            .map(|s| collapse_ws(s));
        let image_url = meta
            .get("og:image")
            .or_else(|| meta.get("twitter:image"))
            .or_else(|| meta.get("twitter:image:src"))
            .map(|s| resolve_url(&url, s));
        let host = url_host(&url).to_string();

        if title.is_none() && description.is_none() && image_url.is_none() {
            return FetchedMedia { url, state: MediaState::Skipped };
        }

        let image = match image_url {
            Some(iu) => fetch_image(&client, &iu).await.ok(),
            None => None,
        };
        return FetchedMedia {
            url,
            state: MediaState::LinkCard { title, description, host, image },
        };
    }

    if ct.starts_with("audio/") {
        return FetchedMedia {
            url,
            state: MediaState::File { kind: MediaKind::Audio, content_type: ct, size },
        };
    }
    if ct.starts_with("video/") {
        return FetchedMedia {
            url,
            state: MediaState::File { kind: MediaKind::Video, content_type: ct, size },
        };
    }

    // Fallback: some servers (paste sites, raw endpoints) return images
    // labelled as text/plain or application/octet-stream. If the size
    // looks plausible, try decoding the body as an image — if it parses,
    // render it as one.
    let ct_unknown = ct.is_empty()
        || ct == "application/octet-stream"
        || ct.starts_with("text/plain");
    let too_big = size.map_or(false, |s| s > MAX_IMAGE_BYTES);
    if ct_unknown && !too_big {
        if let Ok((handle, w, h)) = fetch_image(&client, &url).await {
            return FetchedMedia { url, state: MediaState::Image { handle, w, h } };
        }
    }

    FetchedMedia { url, state: MediaState::Skipped }
}

async fn fetch_image(
    client: &reqwest::Client,
    url: &str,
) -> Result<(iced_image::Handle, u32, u32), String> {
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("image too large".into());
    }
    let (w, h) = image::load_from_memory(&bytes)
        .map(|img| (img.width(), img.height()))
        .map_err(|e| format!("decode: {e}"))?;
    Ok((iced_image::Handle::from_bytes(bytes.to_vec()), w, h))
}

async fn fetch_html_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > 4 * MAX_HTML_BYTES as u64 {
            return Err(format!("html too large ({len} bytes)"));
        }
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let slice = &bytes[..bytes.len().min(MAX_HTML_BYTES)];
    Ok(String::from_utf8_lossy(slice).into_owned())
}

/// Extracts og:*, twitter:*, and <title> from raw HTML using a tiny
/// hand-rolled parser. Robust enough for well-formed meta tags; not a
/// real HTML parser.
fn parse_html_meta(html: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let lower = html.to_ascii_lowercase();

    // <title>…</title>
    if let Some(start) = lower.find("<title") {
        if let Some(gt) = lower[start..].find('>') {
            let after = start + gt + 1;
            if let Some(end_rel) = lower[after..].find("</title>") {
                let raw = &html[after..after + end_rel];
                out.insert("title".into(), html_decode(raw));
            }
        }
    }

    // <meta …>
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("<meta") {
        let start = i + rel;
        let after_tag = start + 5;
        if after_tag >= bytes.len() {
            break;
        }
        // tag must have whitespace or '/' or '>' right after "<meta"
        let next = bytes[after_tag];
        if !next.is_ascii_whitespace() && next != b'/' && next != b'>' {
            i = after_tag;
            continue;
        }
        let close = match lower[after_tag..].find('>') {
            Some(p) => after_tag + p,
            None => break,
        };
        let attrs = &html[after_tag..close];
        i = close + 1;

        let key = extract_attr(attrs, "property")
            .or_else(|| extract_attr(attrs, "name"))
            .map(|s| s.to_ascii_lowercase());
        let content = extract_attr(attrs, "content");
        if let (Some(k), Some(v)) = (key, content) {
            // Keep og:*, twitter:*, and the bare description.
            if k.starts_with("og:") || k.starts_with("twitter:") || k == "description" {
                out.entry(k).or_insert(html_decode(&v));
            }
        }
    }
    out
}

/// Finds the first `<img src="...">` in the document and returns the
/// src attribute. Used as a fallback when a page has no OpenGraph
/// metadata but embeds an image (paste sites, raw wrappers, etc.).
fn extract_first_img_src(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("<img") {
        let start = i + rel;
        let after_tag = start + 4;
        if after_tag >= bytes.len() {
            return None;
        }
        let next = bytes[after_tag];
        if !next.is_ascii_whitespace() && next != b'/' && next != b'>' {
            i = after_tag;
            continue;
        }
        let close = match lower[after_tag..].find('>') {
            Some(p) => after_tag + p,
            None => return None,
        };
        let attrs = &html[after_tag..close];
        if let Some(src) = extract_attr(attrs, "src") {
            if !src.is_empty() {
                return Some(src);
            }
        }
        i = close + 1;
    }
    None
}

/// Pulls a single attribute's quoted value from a meta-tag's attribute
/// substring. Tolerates either single or double quotes and handles a
/// few attributes appearing in any order.
fn extract_attr(attrs: &str, name: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find(name) {
        let pos = from + rel;
        // must be at start or preceded by whitespace
        let prev_ok = pos == 0
            || attrs.as_bytes()[pos - 1].is_ascii_whitespace()
            || attrs.as_bytes()[pos - 1] == b'/';
        let after = pos + name.len();
        if !prev_ok || after >= attrs.len() {
            from = after;
            continue;
        }
        // skip whitespace, then expect '='
        let bytes = attrs.as_bytes();
        let mut j = after;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            from = after;
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            return None;
        }
        let q = bytes[j];
        if q == b'"' || q == b'\'' {
            j += 1;
            let end = attrs[j..].find(q as char).map(|p| j + p)?;
            return Some(attrs[j..end].to_string());
        } else {
            let end = attrs[j..]
                .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
                .map(|p| j + p)
                .unwrap_or(attrs.len());
            return Some(attrs[j..end].to_string());
        }
    }
    None
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_ws = false;
    for c in s.trim().chars() {
        let ws = c.is_whitespace();
        if ws {
            if !prev_ws {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        prev_ws = ws;
    }
    out
}

/// Resolves a possibly-relative URL against a base. Handles absolute
/// (https://, http://), protocol-relative (//host/...), root-relative
/// (/path), and same-dir relative paths well enough for og:image use.
fn resolve_url(base: &str, target: &str) -> String {
    let t = target.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        return t.to_string();
    }
    if let Some(rest) = t.strip_prefix("//") {
        let scheme = if base.starts_with("http://") { "http" } else { "https" };
        return format!("{scheme}://{rest}");
    }
    let scheme = if base.starts_with("http://") { "http" } else { "https" };
    let host = url_host(base);
    if let Some(rest) = t.strip_prefix('/') {
        return format!("{scheme}://{host}/{rest}");
    }
    // same-dir relative: drop last path segment of base.
    let path_start = base.find("://").map(|p| p + 3).unwrap_or(0);
    let after_host = base[path_start..]
        .find('/')
        .map(|p| path_start + p)
        .unwrap_or(base.len());
    let dir_end = base[..after_host].len()
        + base[after_host..]
            .rfind('/')
            .map(|p| p + 1)
            .unwrap_or(1);
    format!("{}{}", &base[..dir_end.min(base.len())], t)
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn url_host(url: &str) -> &str {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host_end = after_scheme.find(['/', '?', '#']).unwrap_or(after_scheme.len());
    &after_scheme[..host_end]
}

fn url_filename(url: &str) -> &str {
    let path = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(path)
}

fn image_preview<'a>(
    url: &str,
    handle: iced_image::Handle,
    w: u32,
    h: u32,
    alpha: f32,
) -> Element<'a, Message> {
    let scale = (MAX_PREVIEW_W / w as f32)
        .min(MAX_PREVIEW_H / h as f32)
        .min(1.0);
    let display_w = (w as f32 * scale).max(1.0);
    let display_h = (h as f32 * scale).max(1.0);
    let img = container(
        iced_image(handle)
            .width(Length::Fixed(display_w))
            .height(Length::Fixed(display_h))
            .content_fit(ContentFit::Contain)
            .opacity(alpha),
    )
    .padding(0)
    .style(move |_| container::Style {
        background: None,
        border: Border { radius: 8.0.into(), ..Default::default() },
        ..Default::default()
    })
    .clip(true);
    let url = url.to_string();
    mouse_area(img)
        .on_press(Message::OpenUrl(url))
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn file_card<'a>(
    url: &str,
    kind: MediaKind,
    content_type: &str,
    size: Option<u64>,
    alpha: f32,
) -> Element<'a, Message> {
    let icon = match kind {
        MediaKind::Audio => "♪",
        MediaKind::Video => "▶",
    };
    let title = url_filename(url).to_string();
    let host = url_host(url).to_string();
    let url_owned = url.to_string();

    let meta = match size {
        Some(s) => format!("{} · {} · {}", content_type, human_size(s), host),
        None => format!("{content_type} · {host}"),
    };

    let card = container(
        row![
            container(text(icon).size(sz(20.0)).color(Color { a: alpha, ..tok::text() }))
                .width(Length::Fixed(40.0))
                .height(Length::Fixed(40.0))
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center)
                .style(move |_| container::Style {
                    background: Some(Background::Color(tok::bg_hover())),
                    border: Border { radius: 6.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            column![
                text(truncate(&title, 64))
                    .size(sz(13.0))
                    .color(Color { a: alpha, ..tok::text() })
                    .font(medium())
                    .wrapping(iced::widget::text::Wrapping::None),
                text(meta)
                    .size(sz(11.0))
                    .color(Color { a: alpha * 0.85, ..tok::text_faint() })
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(2)
        ]
        .spacing(tok::S3)
        .align_y(iced::Alignment::Center),
    )
    .padding(pad(tok::S3 as f32, tok::S3 as f32, tok::S3 as f32, tok::S3 as f32))
    .max_width(MAX_PREVIEW_W)
    .style(move |_| container::Style {
        background: Some(Background::Color(tok::bg_elev())),
        border: Border {
            color: tok::border_soft(),
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    });
    mouse_area(card)
        .on_press(Message::OpenUrl(url_owned))
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn link_card<'a>(
    url: &str,
    title: Option<&str>,
    description: Option<&str>,
    host: &str,
    image: &Option<(iced_image::Handle, u32, u32)>,
    alpha: f32,
) -> Element<'a, Message> {
    let url_owned = url.to_string();
    use iced::widget::text::Wrapping;

    // Hide the title if any meaningful slice of it shows up in the
    // description (common on GitHub repos where og:title is
    // "Owner/repo: <description>" and og:description repeats the same
    // text). Check both halves of the title around an optional colon.
    let title = title.filter(|t| {
        let t = t.trim();
        if t.is_empty() {
            return false;
        }
        let Some(d) = description else { return true };
        let d_lower = d.to_lowercase();
        let t_lower = t.to_lowercase();
        let chunks: Vec<&str> = t_lower
            .split(':')
            .map(str::trim)
            .filter(|s| s.len() >= 12)
            .collect();
        // If any chunk's first 24 chars appear in description, the
        // title isn't adding information.
        for c in chunks {
            let head: String = c.chars().take(24).collect();
            if d_lower.contains(&head) {
                return false;
            }
        }
        true
    });

    let header = text(host.to_string())
        .size(sz(11.0))
        .color(Color { a: alpha * 0.85, ..tok::text_faint() })
        .font(medium())
        .wrapping(Wrapping::None);

    let mut body = column![header].spacing(2).width(Length::Fill);
    if let Some(t) = title {
        body = body.push(
            text(truncate(t, 70))
                .size(sz(13.0))
                .color(Color { a: alpha, ..tok::text() })
                .font(medium())
                .wrapping(Wrapping::Word)
                .width(Length::Fill),
        );
    }
    if let Some(d) = description.filter(|s| !s.is_empty()) {
        body = body.push(
            text(truncate(d, 200))
                .size(sz(11.0))
                .color(Color { a: alpha * 0.9, ..tok::text_muted() })
                .wrapping(Wrapping::Word)
                .width(Length::Fill),
        );
    }

    let card_inner: Element<Message> = match image {
        Some((handle, w, h)) => {
            // Aspect-aware thumbnail: max 128x96, never upscaled. Wide
            // images (GitHub, YouTube) get ~128x68; tall images
            // (vertical photos) get ~64x96; square images get ~96x96.
            const MAX_W: f32 = 128.0;
            const MAX_H: f32 = 96.0;
            let scale = (MAX_W / *w as f32).min(MAX_H / *h as f32).min(1.0);
            let dw = (*w as f32 * scale).max(1.0);
            let dh = (*h as f32 * scale).max(1.0);
            let thumb = container(
                iced_image(handle.clone())
                    .width(Length::Fixed(dw))
                    .height(Length::Fixed(dh))
                    .content_fit(ContentFit::Contain)
                    .opacity(alpha),
            )
            .width(Length::Fixed(dw))
            .height(Length::Fixed(dh))
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_hover())),
                border: Border { radius: 6.0.into(), ..Default::default() },
                ..Default::default()
            })
            .clip(true);
            row![body, thumb]
                .spacing(tok::S3)
                .align_y(iced::Alignment::Start)
                .into()
        }
        None => body.into(),
    };

    let card = container(card_inner)
        .padding(pad(tok::S3 as f32, tok::S3 as f32, tok::S3 as f32, tok::S3 as f32))
        .max_width(MAX_PREVIEW_W)
        .style(move |_| container::Style {
            background: Some(Background::Color(tok::bg_elev())),
            border: Border {
                color: tok::border_soft(),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .clip(true);
    mouse_area(card)
        .on_press(Message::OpenUrl(url_owned))
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

fn media_error<'a>(url: &str, msg: &str, alpha: f32) -> Element<'a, Message> {
    let host = url_host(url).to_string();
    container(
        text(format!("{host} · {msg}"))
            .size(sz(11.0))
            .color(Color { a: alpha * 0.7, ..tok::text_faint() })
            .wrapping(iced::widget::text::Wrapping::None),
    )
    .padding(pad(2.0, 0.0, 2.0, 0.0))
    .into()
}

fn last_word_start(s: &str) -> usize {
    s.char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

fn compose_placeholder(channel_name: &str) -> String {
    if channel_name.starts_with('&') {
        "send a message…".into()
    } else if channel_name.starts_with('#') {
        format!("message {channel_name}…")
    } else {
        format!("message @{channel_name}…")
    }
}

fn send_button_style(enabled: bool, status: button::Status) -> button::Style {
    let bg = if !enabled {
        tok::bg_hover()
    } else {
        match status {
            button::Status::Hovered => Color {
                r: tok::accent().r * 0.88,
                g: tok::accent().g * 0.88,
                b: tok::accent().b * 0.98,
                a: 1.0,
            },
            button::Status::Pressed => Color {
                r: tok::accent().r * 0.78,
                g: tok::accent().g * 0.78,
                b: tok::accent().b * 0.94,
                a: 1.0,
            },
            _ => tok::accent(),
        }
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: if enabled { Color::WHITE } else { tok::text_faint() },
        border: Border { radius: 10.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn ghost_button_style(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => tok::bg_hover(),
        button::Status::Pressed => tok::bg_2(),
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text_mid(),
        border: Border { radius: 10.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn close_btn_style(status: button::Status, base_alpha: f32) -> button::Style {
    let bg_alpha = match status {
        button::Status::Hovered => 0.18 * base_alpha,
        _ => 0.0,
    };
    let bg = Color { a: bg_alpha, ..tok::text_faint() };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text_mid(),
        border: Border { radius: 4.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn member_row_style(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => tok::bg_hover(),
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text(),
        border: Border { radius: 5.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn settings_section_header(label: &str) -> Element<'_, Message> {
    text(label.to_string())
        .size(sz(14.0))
        .font(medium())
        .color(tok::text())
        .into()
}

fn settings_row<'a>(
    label: &'a str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    row![
        container(
            text(label.to_string())
                .size(sz(11.0))
                .font(medium())
                .color(tok::text_mid())
        )
        .width(Length::Fixed(130.0))
        .padding(pad(tok::S2 as f32, 0.0, 0.0, 0.0)),
        control,
    ]
    .spacing(tok::S3)
    .align_y(iced::Alignment::Start)
    .into()
}

fn settings_text_input<'a>(
    placeholder: &'a str,
    value: String,
    field: NetField,
) -> Element<'a, Message> {
    text_input(placeholder, &value)
        .on_input(move |v| Message::SettingsNetField(field, v))
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .size(sz(11.0))
        .width(Length::Fixed(320.0))
        .style(|_, status| input_style(status))
        .into()
}

fn settings_password_input<'a>(
    placeholder: &'a str,
    value: String,
    field: NetField,
) -> Element<'a, Message> {
    text_input(placeholder, &value)
        .on_input(move |v| Message::SettingsNetField(field, v))
        .secure(true)
        .padding(pad(tok::S2 as f32, tok::S3, tok::S2 as f32, tok::S3))
        .size(sz(11.0))
        .width(Length::Fixed(320.0))
        .style(|_, status| input_style(status))
        .into()
}

fn chip<'a>(label: &str, on_remove: Message) -> Element<'a, Message> {
    let inner = row![
        text(label.to_string())
            .size(sz(11.0))
            .color(tok::text())
            .font(regular()),
        button(text("×").size(sz(11.0)).color(tok::text_faint()))
            .padding(pad(0.0, 4.0, 0.0, 4.0))
            .style(|_, status| ghost_btn_style(status))
            .on_press(on_remove),
    ]
    .spacing(tok::S1)
    .align_y(iced::Alignment::Center);
    container(inner)
        .padding(pad(2.0, tok::S2, 2.0, tok::S3))
        .style(|_| container::Style {
            background: Some(Background::Color(tok::bg_2())),
            border: Border {
                color: tok::border_soft(),
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .into()
}

// Builds a flowing list of chips by stacking horizontal rows that each
// hold up to a fixed count. Cheap-and-cheerful wrap since iced 0.14 has
// no native wrap container.
fn wrap_row<'a>(items: Vec<Element<'a, Message>>, _gap: f32) -> Element<'a, Message> {
    const PER_ROW: usize = 5;
    let mut rows_v: Vec<Element<'a, Message>> = Vec::new();
    let mut current: Vec<Element<'a, Message>> = Vec::with_capacity(PER_ROW);
    for el in items {
        current.push(el);
        if current.len() >= PER_ROW {
            let taken = std::mem::take(&mut current);
            rows_v.push(row(taken).spacing(tok::S2).into());
        }
    }
    if !current.is_empty() {
        rows_v.push(row(current).spacing(tok::S2).into());
    }
    column(rows_v).spacing(tok::S2).into()
}

fn primary_btn_style(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => tok::accent(),
        button::Status::Pressed => tok::accent_ring(),
        _ => tok::accent(),
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: Color::WHITE,
        border: Border { radius: 6.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn ghost_btn_style(status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => tok::bg_hover(),
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text(),
        border: Border { radius: 6.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn section_btn_style(selected: bool, status: button::Status) -> button::Style {
    let bg = if selected {
        tok::bg_hover()
    } else {
        match status {
            button::Status::Hovered => tok::bg_hover(),
            _ => Color::TRANSPARENT,
        }
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text(),
        border: Border { radius: 6.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn palette_row_style(selected: bool, status: button::Status) -> button::Style {
    let bg = if selected {
        tok::accent_soft()
    } else {
        match status {
            button::Status::Hovered => tok::bg_hover(),
            _ => Color::TRANSPARENT,
        }
    };
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: tok::text(),
        border: Border { radius: 4.0.into(), ..Default::default() },
        shadow: Shadow::default(),
        ..Default::default()
    }
}

fn status_color(status: ConnStatus) -> Color {
    match status {
        ConnStatus::Connected => Color::from_rgb(0.40, 0.80, 0.55),
        ConnStatus::Connecting => Color::from_rgb(0.98, 0.78, 0.42),
        ConnStatus::Disconnected => Color::from_rgb(0.60, 0.60, 0.65),
        ConnStatus::Error => Color::from_rgb(0.95, 0.50, 0.50),
        ConnStatus::TemplateWritten | ConnStatus::NotConfigured => {
            Color::from_rgb(0.55, 0.55, 0.62)
        }
    }
}

fn notify(title: String, body: String) {
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .summary(&title)
            .body(&body)
            .appname("Murmur")
            .show();
    });
}

fn mentions(body: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let lower = body.to_lowercase();
    let needle = nick.to_lowercase();
    let bytes = lower.as_bytes();
    let nbytes = needle.as_bytes();
    if nbytes.is_empty() || nbytes.len() > bytes.len() {
        return false;
    }
    for i in 0..=bytes.len() - nbytes.len() {
        if &bytes[i..i + nbytes.len()] == nbytes {
            let before = if i == 0 { b' ' } else { bytes[i - 1] };
            let after_idx = i + nbytes.len();
            let after = if after_idx >= bytes.len() { b' ' } else { bytes[after_idx] };
            let word_boundary = |c: u8| !c.is_ascii_alphanumeric() && c != b'_';
            if word_boundary(before) && word_boundary(after) {
                return true;
            }
        }
    }
    false
}

fn nick_color(nick: &str) -> Color {
    let palette = [
        Color::from_rgb(0.95, 0.65, 0.68), // rose
        Color::from_rgb(0.70, 0.88, 0.72), // green
        Color::from_rgb(0.72, 0.80, 1.00), // blue
        Color::from_rgb(0.98, 0.82, 0.60), // amber
        Color::from_rgb(0.87, 0.72, 0.95), // lilac
        Color::from_rgb(0.65, 0.90, 0.92), // cyan
        Color::from_rgb(0.95, 0.75, 0.80), // pink
    ];
    if nick == "*" {
        return tok::text_muted();
    }
    let idx = nick.bytes().fold(0u32, |a, b| a.wrapping_add(b as u32)) as usize % palette.len();
    palette[idx]
}
