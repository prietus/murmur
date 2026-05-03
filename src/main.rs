mod config;
mod irc_worker;

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::path::PathBuf;
use std::time::Instant;

use std::pin::Pin;

use futures::channel::mpsc;
use futures::Stream;
use iced::animation::{Animation, Easing};
use iced::keyboard;
use iced::widget::{
    button, column, container, image as iced_image, mouse_area, row, scrollable, stack, text,
    text_input, Space,
};
use iced::ContentFit;
use iced::{
    window, Background, Border, Color, Element, Fill, Font, Length, Padding, Shadow, Subscription,
    Task, Theme,
};

use crate::config::{AppConfig, LoadResult};
use crate::irc_worker::{Event as IrcEvent, Outgoing};

const FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const FONT_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
const FONT_NAME: &str = "JetBrains Mono";

static USER_FONT: OnceLock<&'static str> = OnceLock::new();
static FONT_SCALE: OnceLock<f32> = OnceLock::new();

const FADE_MS: u128 = 250;
const GROUP_SECS: u64 = 300;

const SIDEBAR_W: f32 = 180.0;
const MEMBERS_W: f32 = 140.0;
const CHAT_MAX_W: f32 = 880.0;

const PALETTE_INPUT_ID: &str = "palette-input";
const COMPOSE_INPUT_ID: &str = "compose-input";
const PALETTE_W: f32 = 520.0;
const PALETTE_MAX_ITEMS: usize = 8;

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
];

fn main() -> iced::Result {
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
    base * font_scale()
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

struct TileStyle {
    bg: Color,
    fg: Color,
    border: Color,
    border_width: f32,
}

fn tile_style_for(hover_v: f32, select_v: f32) -> TileStyle {
    let bg = blend(
        blend(tok::bg_elev(), tok::bg_hover(), hover_v),
        tok::accent_soft(),
        select_v,
    );
    let fg = blend(
        blend(tok::text_mid(), tok::text(), hover_v),
        tok::accent(),
        select_v,
    );
    let border = blend(
        blend(tok::border_soft(), tok::border(), hover_v),
        Color::TRANSPARENT,
        select_v,
    );
    TileStyle {
        bg,
        fg,
        border,
        border_width: 1.0 * (1.0 - select_v),
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

fn channel_tile(
    prefix: &'static str,
    style: TileStyle,
    side: f32,
    font_size: f32,
) -> Element<'static, Message> {
    let TileStyle { bg, fg, border, border_width } = style;
    container(text(prefix).size(font_size).color(fg).font(medium()))
        .width(Length::Fixed(side))
        .height(Length::Fixed(side))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(bg)),
            border: Border {
                color: border,
                width: border_width,
                radius: (side * 0.28).into(),
            },
            ..Default::default()
        })
        .into()
}

#[derive(Clone)]
enum Message {
    ChannelSelected(usize),
    InputChanged(String),
    SendMessage,
    ToggleSidebar,
    ToggleMembers,
    Tick(Instant),
    Irc(IrcEvent),
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
enum ConnStatus {
    NotConfigured,
    TemplateWritten,
    Connecting,
    Connected,
    Disconnected,
    Error,
}

struct App {
    channels: Vec<Channel>,
    selected: usize,
    input: String,
    now: Instant,
    sidebar_anim: Animation<bool>,
    members_anim: Animation<bool>,
    cfg: Option<AppConfig>,
    #[allow(dead_code)]
    cfg_path: Option<PathBuf>,
    status: ConnStatus,
    last_error: Option<String>,
    outgoing: Option<mpsc::Sender<Outgoing>>,
    dimmed_nicks: HashSet<String>,
    palette_open: bool,
    palette_query: String,
    palette_cursor: usize,
    hovered_channel: Option<usize>,
    hovered_member: Option<usize>,
    tab_state: Option<TabState>,
    media_cache: HashMap<String, MediaState>,
    theme_name: String,
    input_history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
}

struct TabState {
    word_start: usize,
    matches: Vec<String>,
    idx: usize,
    suffix: &'static str,
    expected_input: String,
}

struct Channel {
    name: String,
    topic: Option<String>,
    messages: Vec<ChatMessage>,
    members: Vec<String>,
    dimm: bool,
    hide_joinpart: bool,
    hover_anim: Animation<bool>,
    select_anim: Animation<bool>,
    fade_baseline: Instant,
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
}

impl Default for App {
    fn default() -> Self {
        let now = Instant::now();
        let base = App {
            channels: Vec::new(),
            selected: 0,
            input: String::new(),
            now,
            sidebar_anim: Animation::new(true).quick().easing(Easing::EaseOutQuint),
            members_anim: Animation::new(true).quick().easing(Easing::EaseOutQuint),
            cfg: None,
            cfg_path: config::config_path(),
            status: ConnStatus::NotConfigured,
            last_error: None,
            outgoing: None,
            dimmed_nicks: HashSet::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_cursor: 0,
            hovered_channel: None,
            hovered_member: None,
            tab_state: None,
            media_cache: HashMap::new(),
            theme_name: "soft-dark".into(),
            input_history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
        };

        match config::load() {
            LoadResult::Loaded(cfg) => {
                let theme_name = cfg
                    .theme
                    .clone()
                    .filter(|n| themes::by_name(n).is_some())
                    .unwrap_or_else(|| "soft-dark".into());
                theme::set(themes::by_name(&theme_name).unwrap_or(themes::SOFT_DARK));
                App {
                    channels: vec![status_channel(
                        "",
                        vec![system_line(
                            &format!("connecting to {}:{}...", cfg.server, cfg.port),
                            now,
                        )],
                    )],
                    status: ConnStatus::Connecting,
                    cfg: Some(cfg),
                    theme_name,
                    ..base
                }
            }
            LoadResult::WroteTemplate(path) => App {
                channels: vec![status_channel(
                    "",
                    vec![
                        system_line("no config found", now),
                        system_line(&format!("wrote template at {}", path.display()), now),
                        system_line("edit it with your server + nick, then restart.", now),
                    ],
                )],
                status: ConnStatus::TemplateWritten,
                cfg_path: Some(path),
                ..base
            },
            LoadResult::Error(e) => App {
                channels: vec![status_channel(
                    "",
                    vec![
                        system_line("config error:", now),
                        system_line(&e, now),
                    ],
                )],
                status: ConnStatus::Error,
                last_error: Some(e),
                ..base
            },
        }
    }
}

fn status_channel(topic: &str, messages: Vec<ChatMessage>) -> Channel {
    Channel {
        name: "&status".into(),
        topic: if topic.is_empty() { None } else { Some(topic.into()) },
        members: Vec::new(),
        messages,
        dimm: false,
        hide_joinpart: false,
        hover_anim: new_row_anim(),
        select_anim: new_row_anim(),
        fade_baseline: Instant::now(),
    }
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
    }
}

fn irc_sub(cfg: &AppConfig) -> Pin<Box<dyn Stream<Item = IrcEvent> + Send>> {
    Box::pin(irc_worker::subscribe(cfg))
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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let secs = (now % 60) as u32;
        let mins = ((now / 60) % 60) as u32;
        let hrs = ((now / 3600) % 24) as u32;
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
        .unwrap_or(0);
    let h = (s / 3600) % 24;
    let m = (s / 60) % 60;
    format!("{h:02}:{m:02}")
}

impl App {
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

                if let Some(cmd) = text.strip_prefix('/') {
                    self.handle_command(cmd);
                    return Task::none();
                }

                let target = self.channels[self.selected].name.clone();
                let is_status = target.starts_with('&');

                if !is_status {
                    if let Some(tx) = self.outgoing.as_mut() {
                        let _ = tx.try_send(Outgoing::Privmsg {
                            target: target.clone(),
                            text: text.clone(),
                        });
                    }
                    let nick = self
                        .cfg
                        .as_ref()
                        .map(|c| c.nickname.clone())
                        .unwrap_or_else(|| "you".into());
                    let now = Instant::now();
                    let fetch = self.schedule_media_fetches(&text);
                    chatlog::append(
                        &self.server_for_log(),
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
            Message::Irc(event) => self.handle_irc(event),
            Message::MediaFetched(fetched) => {
                self.media_cache.insert(fetched.url, fetched.state);
                Task::none()
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
                    if let Some(tx) = self.outgoing.as_mut() {
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
        }
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

    fn server_for_log(&self) -> String {
        self.cfg
            .as_ref()
            .map(|c| c.server.clone())
            .unwrap_or_else(|| "unknown".into())
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
        let channel = if target.starts_with('#') || target.starts_with('&') {
            target.to_string()
        } else {
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
        let nick = self
            .cfg
            .as_ref()
            .map(|c| c.nickname.clone())
            .unwrap_or_else(|| "you".into());
        self.channels[self.selected].messages.push(ChatMessage {
            nick,
            body: text,
            time: now_hhmm(),
            day: "today".into(),
            inserted_at: now,
            mono_secs: now.elapsed().as_secs(),
            kind: MsgKind::Action,
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
        let nick = self
            .cfg
            .as_ref()
            .map(|c| c.nickname.clone())
            .unwrap_or_else(|| "you".into());
        self.channels[idx].messages.push(ChatMessage {
            nick,
            body,
            time: now_hhmm(),
            day: "today".into(),
            inserted_at: now,
            mono_secs: now.elapsed().as_secs(),
            kind: MsgKind::Chat,
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
        match self.outgoing.as_mut() {
            Some(tx) => match tx.try_send(msg) {
                Ok(()) => true,
                Err(_) => {
                    self.channels[self.selected]
                        .messages
                        .push(system_line("send failed — channel full or closed", now));
                    false
                }
            },
            None => {
                self.channels[self.selected]
                    .messages
                    .push(system_line("not connected", now));
                false
            }
        }
    }

    fn handle_irc(&mut self, event: IrcEvent) -> Task<Message> {
        let now = Instant::now();
        self.now = now;
        match event {
            IrcEvent::Ready(tx) => {
                self.outgoing = Some(tx);
                Task::none()
            }
            IrcEvent::Connected => {
                self.status = ConnStatus::Connected;
                self.push_status(system_line("connected", now));
                Task::none()
            }
            IrcEvent::ConnectError(e) => {
                self.status = ConnStatus::Error;
                self.push_status(system_line(&format!("error: {e}"), now));
                self.last_error = Some(e);
                Task::none()
            }
            IrcEvent::Disconnected => {
                self.status = ConnStatus::Disconnected;
                self.push_status(system_line("disconnected", now));
                Task::none()
            }
            IrcEvent::Privmsg { target, nick, body } => {
                let my_nick = self
                    .cfg
                    .as_ref()
                    .map(|c| c.nickname.clone())
                    .unwrap_or_default();
                let is_self = nick == my_nick;
                let is_dm = !my_nick.is_empty() && target == my_nick;
                if !is_self {
                    if is_dm {
                        notify(format!("@{nick}"), body.clone());
                    } else if mentions(&body, &my_nick) {
                        notify(format!("{target} — {nick}"), body.clone());
                    }
                }
                let bucket = if is_dm { nick.clone() } else { target };
                let idx = self.ensure_channel(&bucket);
                let fetch = self.schedule_media_fetches(&body);
                chatlog::append(
                    &self.server_for_log(),
                    &bucket,
                    &format!("{}  <{}> {}", chatlog::iso_now(), nick, body),
                );
                self.channels[idx].messages.push(ChatMessage {
                    nick,
                    body,
                    time: now_hhmm(),
                    day: "today".into(),
                    inserted_at: now,
                    mono_secs: now.elapsed().as_secs(),
                    kind: MsgKind::Chat,
                });
                fetch
            }
            IrcEvent::Action { target, nick, body } => {
                let my_nick = self
                    .cfg
                    .as_ref()
                    .map(|c| c.nickname.clone())
                    .unwrap_or_default();
                let is_self = nick == my_nick;
                let is_dm = !my_nick.is_empty() && target == my_nick;
                if !is_self {
                    if is_dm {
                        notify(format!("@{nick}"), format!("{nick} {body}"));
                    } else if mentions(&body, &my_nick) {
                        notify(format!("{target} — {nick}"), format!("{nick} {body}"));
                    }
                }
                let bucket = if is_dm { nick.clone() } else { target };
                let idx = self.ensure_channel(&bucket);
                let fetch = self.schedule_media_fetches(&body);
                chatlog::append(
                    &self.server_for_log(),
                    &bucket,
                    &format!("{}  * {} {}", chatlog::iso_now(), nick, body),
                );
                self.channels[idx].messages.push(ChatMessage {
                    nick,
                    body,
                    time: now_hhmm(),
                    day: "today".into(),
                    inserted_at: now,
                    mono_secs: now.elapsed().as_secs(),
                    kind: MsgKind::Action,
                });
                fetch
            }
            IrcEvent::NickChanged { old, new } => {
                let is_self = self
                    .cfg
                    .as_ref()
                    .is_some_and(|c| c.nickname == old);
                if is_self {
                    if let Some(cfg) = self.cfg.as_mut() {
                        cfg.nickname = new.clone();
                    }
                }
                for ch in self.channels.iter_mut() {
                    if let Some(pos) = ch.members.iter().position(|n| n == &old) {
                        ch.members[pos] = new.clone();
                        let body = if is_self {
                            format!("you are now {new}")
                        } else {
                            format!("{old} is now {new}")
                        };
                        ch.messages.push(joinpart_line(&body, now));
                    }
                }
                if is_self {
                    self.push_status(system_line(&format!("you are now {new}"), now));
                }
                Task::none()
            }
            IrcEvent::UserJoined { channel, nick } => {
                let idx = self.ensure_channel(&channel);
                if !self.channels[idx].members.iter().any(|n| n == &nick) {
                    self.channels[idx].members.push(nick.clone());
                }
                chatlog::append(
                    &self.server_for_log(),
                    &channel,
                    &format!("{}  -- {} joined", chatlog::iso_now(), nick),
                );
                self.channels[idx]
                    .messages
                    .push(joinpart_line(&format!("→ {nick} joined"), now));
                Task::none()
            }
            IrcEvent::UserLeft { channel, nick } => {
                let idx = self.ensure_channel(&channel);
                self.channels[idx].members.retain(|n| n != &nick);
                chatlog::append(
                    &self.server_for_log(),
                    &channel,
                    &format!("{}  -- {} left", chatlog::iso_now(), nick),
                );
                self.channels[idx]
                    .messages
                    .push(joinpart_line(&format!("← {nick} left"), now));
                Task::none()
            }
            IrcEvent::Names { channel, nicks } => {
                let idx = self.ensure_channel(&channel);
                for n in nicks {
                    if !self.channels[idx].members.iter().any(|m| m == &n) {
                        self.channels[idx].members.push(n);
                    }
                }
                Task::none()
            }
            IrcEvent::Topic { channel, topic } => {
                let idx = self.ensure_channel(&channel);
                chatlog::append(
                    &self.server_for_log(),
                    &channel,
                    &format!("{}  -- topic: {}", chatlog::iso_now(), topic),
                );
                self.channels[idx].topic = Some(topic);
                Task::none()
            }
            IrcEvent::Notice { from, text } => {
                self.push_status(system_line(&format!("-{from}- {text}"), now));
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
                let idx = self.ensure_channel(&from);
                self.channels[idx].messages.push(system_line(&body, now));
                Task::none()
            }
        }
    }

    fn ensure_channel(&mut self, name: &str) -> usize {
        if let Some(i) = self.channels.iter().position(|c| c.name == name) {
            return i;
        }
        self.channels.push(Channel {
            name: name.to_string(),
            topic: None,
            messages: Vec::new(),
            members: Vec::new(),
            dimm: false,
            hide_joinpart: false,
            hover_anim: new_row_anim(),
            select_anim: new_row_anim(),
            fade_baseline: Instant::now(),
        });
        self.channels.len() - 1
    }

    fn set_selected(&mut self, i: usize) {
        if self.selected != i || self.channels[i].fade_baseline.elapsed().as_millis() > FADE_MS * 2 {
            self.channels[i].fade_baseline = Instant::now();
        }
        self.selected = i;
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

    fn push_status(&mut self, msg: ChatMessage) {
        let idx = self.ensure_channel("&status");
        self.channels[idx].messages.push(msg);
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
        if let Some(cfg) = self.cfg.as_ref() {
            subs.push(Subscription::run_with(cfg.clone(), irc_sub).map(Message::Irc));
        }
        subs.push(keyboard::listen().map(Message::Key));
        Subscription::batch(subs)
    }

    fn view(&self) -> Element<'_, Message> {
        let sw = self.sidebar_anim.interpolate(0.0, SIDEBAR_W, self.now);
        let mw = self.members_anim.interpolate(0.0, MEMBERS_W, self.now);

        let mut panes: Vec<Element<Message>> = Vec::with_capacity(3);
        if sw > 0.5 {
            panes.push(self.sidebar(sw));
        }
        panes.push(self.chat_pane());
        if mw > 0.5 {
            panes.push(self.member_pane(mw));
        }

        let main: Element<Message> = row(panes).spacing(0).height(Fill).into();

        if self.palette_open {
            stack![main, self.palette_overlay()].into()
        } else {
            main
        }
    }

    fn palette_overlay(&self) -> Element<'_, Message> {
        let mut items = self.filtered_palette_items();
        items.truncate(PALETTE_MAX_ITEMS);
        let visible = items;

        let input = text_input("type to search, or /command …", &self.palette_query)
            .id(PALETTE_INPUT_ID)
            .on_input(Message::PaletteQuery)
            .on_submit(Message::PaletteActivate)
            .padding(pad(tok::S3, tok::S4, tok::S3, tok::S4))
            .size(sz(14.0))
            .style(palette_input_style);

        let rows: Vec<Element<Message>> = visible
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
            column(rows).spacing(1).into()
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

    fn sidebar(&self, width: f32) -> Element<'_, Message> {
        let dot_color = status_color(self.status);
        let label = self
            .cfg
            .as_ref()
            .map(|c| c.server.clone())
            .unwrap_or_else(|| "not connected".into());

        let server_label = container(
            row![
                container(sp(6, 6)).style(move |_| container::Style {
                    background: Some(Background::Color(dot_color)),
                    border: Border { radius: 3.0.into(), ..Default::default() },
                    ..Default::default()
                }),
                text(label).size(sz(12.0)).font(medium()).color(tok::text_mid()),
            ]
            .spacing(tok::S2)
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S4, tok::S4, tok::S3, tok::S4));

        let divider = container(sp(Fill, 1))
            .style(|_| container::Style {
                background: Some(Background::Color(tok::border_soft())),
                ..Default::default()
            });

        let items: Vec<Element<Message>> = self
            .channels
            .iter()
            .enumerate()
            .map(|(i, ch)| self.channel_row(i, ch))
            .collect();

        let list = scrollable(
            column(items)
                .spacing(0)
                .padding(pad(tok::S2 as f32, tok::S2 as f32, tok::S2 as f32, tok::S2 as f32)),
        )
        .height(Fill);

        container(
            container(column![server_label, divider, list].spacing(0))
                .width(SIDEBAR_W)
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

        let tile = tile_style_for(hover_v, select_v);
        let label_color = blend(tok::text_mid(), tok::text(), hover_v.max(select_v));
        let row_bg = blend(
            blend(Color::TRANSPARENT, tok::bg_hover(), hover_v),
            tok::accent_soft(),
            select_v,
        );

        let row_content = row![
            channel_tile(prefix, tile, 22.0, sz(11.0)),
            text(truncate(&label, 18))
                .size(sz(13.0))
                .font(if selected { medium() } else { regular() })
                .wrapping(iced::widget::text::Wrapping::None)
                .color(label_color),
        ]
        .spacing(tok::S3)
        .align_y(iced::Alignment::Center);

        let btn = button(row_content)
            .on_press(Message::ChannelSelected(i))
            .width(Fill)
            .padding(pad(tok::S1 as f32, tok::S3 as f32, tok::S1 as f32, tok::S2 as f32))
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(row_bg)),
                text_color: tok::text(),
                border: Border { radius: 6.0.into(), ..Default::default() },
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
        let header_title: Element<Message> = row![
            channel_tile(prefix, tile_style_for(0.0, 1.0), 26.0, sz(13.0)),
            text(label).size(sz(15.0)).font(medium()).color(tok::text()),
        ]
        .spacing(tok::S3)
        .align_y(iced::Alignment::Center)
        .into();
        let header_topic: Element<Message> = match &ch.topic {
            Some(t) => text(t.clone()).size(sz(12.0)).color(tok::text_muted()).into(),
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
        .padding(pad(tok::S4, tok::S4, tok::S4, tok::S4))
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
            container(
                column(msgs)
                    .spacing(0)
                    .padding(pad(tok::S4, tok::S6, tok::S4, tok::S6))
                    .width(Fill),
            )
            .max_width(CHAT_MAX_W)
            .center_x(Fill),
        )
        .height(Fill)
        .width(Fill);

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

        let input = container(
            container(
                row![text_field, send_btn]
                    .spacing(tok::S2)
                    .align_y(iced::Alignment::Center),
            )
            .max_width(CHAT_MAX_W)
            .center_x(Fill),
        )
        .padding(pad(tok::S2, tok::S6, tok::S4, tok::S6));

        container(column![header, msg_area, input])
            .width(Fill)
            .height(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(tok::bg_1())),
                ..Default::default()
            })
            .into()
    }

    fn render_messages<'a>(&'a self, ch: &'a Channel) -> Vec<Element<'a, Message>> {
        let my_nick = self
            .cfg
            .as_ref()
            .map(|c| c.nickname.as_str())
            .unwrap_or("");

        let baseline = ch.fade_baseline;

        let mut out: Vec<Element<Message>> = Vec::with_capacity(ch.messages.len() * 2);
        let mut prev_day: Option<&str> = None;
        let mut prev_nick: Option<&str> = None;
        let mut prev_secs: u64 = 0;

        for m in &ch.messages {
            if prev_day != Some(m.day.as_str()) {
                out.push(self.day_separator(&m.day));
                prev_day = Some(m.day.as_str());
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

            out.push(self.message_line(m, grouped, dim_level, baseline));

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
                sp(44, 0),
                sp(64, 0),
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
    ) -> Element<'a, Message> {
        let start = m.inserted_at.max(baseline);
        let age_ms = start.elapsed().as_millis().min(FADE_MS);
        let t = age_ms as f32 / FADE_MS as f32;
        let fade = 1.0 - (1.0 - t).powi(3); // ease-out cubic
        let alpha = fade * dim_level;

        let nick_color = nick_color(&m.nick);

        let time_el: Element<Message> = if grouped {
            text("").size(sz(11.0)).width(44).into()
        } else {
            text(m.time.clone())
                .size(sz(11.0))
                .color(Color { a: 0.7 * alpha, ..tok::text_faint() })
                .width(44)
                .into()
        };

        let nick_el: Element<Message> = if grouped {
            sp(64, 0).into()
        } else {
            let nick_text = text(truncate(&m.nick, 10))
                .size(sz(13.0))
                .color(Color { a: alpha, ..nick_color })
                .width(64)
                .font(medium())
                .wrapping(iced::widget::text::Wrapping::None);
            let my_nick = self.cfg.as_ref().map(|c| c.nickname.as_str()).unwrap_or("");
            let clickable =
                m.kind != MsgKind::System && !m.nick.is_empty() && m.nick != my_nick;
            if clickable {
                mouse_area(nick_text)
                    .on_press(Message::StartDmWith(m.nick.clone()))
                    .interaction(iced::mouse::Interaction::Pointer)
                    .into()
            } else {
                nick_text.into()
            }
        };

        let (body_font, body_color) = if m.kind == MsgKind::Action {
            (italic(), tok::text_mid())
        } else {
            (regular(), tok::text())
        };
        let body_el = text(m.body.clone())
            .size(sz(13.0))
            .color(Color { a: alpha, ..body_color })
            .font(body_font);

        let top_pad = if grouped { 0.0 } else { tok::S1 as f32 };

        let line_row = row![time_el, nick_el, body_el]
            .spacing(tok::S3)
            .align_y(iced::Alignment::Start);

        let media_els: Vec<Element<Message>> = extract_urls(&m.body)
            .iter()
            .filter_map(|url| match self.media_cache.get(url) {
                Some(MediaState::Image { handle, w, h }) => {
                    Some(image_preview(handle.clone(), *w, *h, alpha))
                }
                Some(MediaState::File { kind, content_type, size }) => {
                    Some(file_card(url, *kind, content_type, *size, alpha))
                }
                Some(MediaState::Error(e)) => Some(media_error(url, e, alpha)),
                _ => None,
            })
            .collect();

        let body: Element<Message> = if media_els.is_empty() {
            line_row.into()
        } else {
            let mut col = column![line_row].spacing(tok::S2);
            let media_indent = 44.0 + 64.0 + tok::S3 as f32 * 2.0;
            for el in media_els {
                col = col.push(
                    row![sp(media_indent, 0), el].align_y(iced::Alignment::Start),
                );
            }
            col.into()
        };

        container(body)
            .padding(pad(top_pad, 0.0, 0.0, 0.0))
            .width(Fill)
            .into()
    }

    fn member_pane(&self, width: f32) -> Element<'_, Message> {
        let ch = &self.channels[self.selected];

        let header = container(
            row![
                text("members").size(sz(11.0)).color(tok::text_muted()).font(medium()),
                sp(Fill, 0),
                text(format!("{}", ch.members.len()))
                    .size(sz(11.0))
                    .color(tok::text_faint())
                    .font(medium()),
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding(pad(tok::S4 as f32, tok::S4 as f32, tok::S3 as f32, tok::S4 as f32));

        let divider = container(sp(Fill, 1)).style(|_| container::Style {
            background: Some(Background::Color(tok::border_soft())),
            ..Default::default()
        });

        let items: Vec<Element<Message>> = ch
            .members
            .iter()
            .enumerate()
            .map(|(i, m)| self.member_row(i, m))
            .collect();

        let list = scrollable(column(items).spacing(0)).height(Fill);

        container(
            container(column![header, divider, list].spacing(0))
                .width(MEMBERS_W)
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

        let row_content = row![
            container(sp(6, 6)).style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgb(0.4, 0.8, 0.55))),
                border: Border { radius: 3.0.into(), ..Default::default() },
                ..Default::default()
            }),
            text(truncate(nick, 14))
                .size(sz(12.0))
                .line_height(iced::widget::text::LineHeight::Absolute(iced::Pixels(sz(14.0))))
                .color(nick_color(nick))
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
            .interaction(iced::mouse::Interaction::Pointer)
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
const MAX_PREVIEW_W: f32 = 480.0;
const MAX_PREVIEW_H: f32 = 360.0;

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
        let bytes = match client.get(&url).send().await {
            Ok(r) => match r.bytes().await {
                Ok(b) => b,
                Err(e) => return make_err(e.to_string()),
            },
            Err(e) => return make_err(e.to_string()),
        };
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return make_err("image too large".into());
        }
        let (w, h) = match image::load_from_memory(&bytes) {
            Ok(img) => (img.width(), img.height()),
            Err(e) => return make_err(format!("decode: {e}")),
        };
        let handle = iced_image::Handle::from_bytes(bytes.to_vec());
        return FetchedMedia { url, state: MediaState::Image { handle, w, h } };
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
    FetchedMedia { url, state: MediaState::Skipped }
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
    container(
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
    .clip(true)
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

    let meta = match size {
        Some(s) => format!("{} · {} · {}", content_type, human_size(s), host),
        None => format!("{content_type} · {host}"),
    };

    container(
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
    })
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
