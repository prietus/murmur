use std::collections::{HashMap, HashSet};

use futures::channel::mpsc;
use futures::{SinkExt, Stream, StreamExt};
use irc::client::prelude::*;
use irc::proto::CapSubCommand;
use irc::proto::message::Tag;

use crate::config::{AuthMode, NetworkConfig};

const WANT_EXTRA_CAPS: &[&str] = &[
    "message-tags",
    "server-time",
    "batch",
    "invite-notify",
    "draft/chathistory",
];

/// Per-message metadata extracted from IRCv3 tags.
#[derive(Clone, Default, Debug)]
pub struct MsgMeta {
    /// HH:MM extracted from the `time` tag if present (UTC).
    pub server_time_hhmm: Option<String>,
    /// Unique server-issued message id from the `msgid` tag.
    pub msgid: Option<String>,
    /// Batch reference tag if this message belongs to an open batch.
    pub batch: Option<String>,
    /// Lower-case batch kind (e.g. "chathistory", "netsplit") looked up
    /// from the open-batch table when the message was processed.
    pub batch_kind: Option<String>,
}

#[derive(Debug, Clone)]
struct BatchInfo {
    kind: String,
    params: Vec<String>,
}

#[derive(Clone)]
pub enum Outgoing {
    Privmsg { target: String, text: String },
    Action { target: String, text: String },
    Ctcp { target: String, query: String },
    Join(String),
    Part { channel: String, reason: Option<String> },
    Nick(String),
    /// Fetch the most recent `limit` messages for `target`. Sent as
    /// `CHATHISTORY LATEST <target> * <limit>`.
    ChatHistoryLatest { target: String, limit: u32 },
    /// `WHOIS <nick>`. Replies arrive as numerics (311/312/317/318/319/330/671).
    Whois(String),
    /// `AWAY :<msg>` to set, `AWAY` (no arg) to clear.
    Away(Option<String>),
    /// `TOPIC <channel> :<topic>` to set; `topic = None` queries.
    Topic { channel: String, topic: Option<String> },
    /// Raw IRC line. `cmd` and `args` already parsed.
    Raw { cmd: String, args: Vec<String> },
}

#[derive(Clone)]
#[allow(dead_code)]
pub enum Event {
    Ready(mpsc::Sender<Outgoing>),
    Connected,
    /// Caps the server actually ACKed during negotiation. Lowercase names.
    CapsAcked(Vec<String>),
    ConnectError(String),
    Disconnected,
    Privmsg { target: String, nick: String, body: String, meta: MsgMeta },
    Action { target: String, nick: String, body: String, meta: MsgMeta },
    UserJoined { channel: String, nick: String, meta: MsgMeta },
    UserLeft { channel: String, nick: String, meta: MsgMeta },
    NickChanged { old: String, new: String, meta: MsgMeta },
    Names { channel: String, nicks: Vec<String> },
    Topic { channel: String, topic: String },
    Notice { from: String, text: String, meta: MsgMeta },
    CtcpReply { from: String, query: String, args: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthPhase {
    AwaitingCapLs,
    AwaitingCapAck,
    AwaitingChallenge,
    AwaitingResult,
    Done,
}

#[derive(Debug, Default)]
struct CapState {
    /// Caps the server announced via CAP LS (key = cap name, value = optional value).
    available: HashSet<String>,
    /// Whether we've seen the final (non-multiline) CAP LS response.
    ls_complete: bool,
    /// Caps the server actually ACKed for this session.
    acked: HashSet<String>,
}

pub fn subscribe(cfg: &NetworkConfig) -> impl Stream<Item = Event> + Send + 'static {
    let cfg = cfg.clone();
    iced::stream::channel(128, move |mut out: mpsc::Sender<Event>| async move {
        let (otx, mut orx) = mpsc::channel::<Outgoing>(64);
        if out.send(Event::Ready(otx)).await.is_err() {
            return;
        }

        let auth_mode = cfg.auth_mode();
        let use_sasl = matches!(auth_mode, AuthMode::SaslPlain | AuthMode::SaslExternal);

        let irc_cfg = Config {
            nickname: Some(cfg.nickname.clone()),
            username: cfg.username.clone(),
            realname: cfg.realname.clone(),
            server: Some(cfg.server.clone()),
            port: Some(cfg.port),
            use_tls: Some(cfg.use_tls),
            channels: cfg.channels.clone(),
            // When SASL is in use we authenticate during connection;
            // skip the post-MOTD NickServ IDENTIFY the crate would do.
            nick_password: if use_sasl { None } else { cfg.nick_password.clone() },
            client_cert_path: cfg.client_cert_path.clone(),
            client_cert_pass: cfg.client_cert_pass.clone(),
            ..Config::default()
        };

        let mut client = match Client::from_config(irc_cfg).await {
            Ok(c) => c,
            Err(e) => {
                let _ = out.send(Event::ConnectError(e.to_string())).await;
                return;
            }
        };

        let sender = client.sender();
        let mut stream = match client.stream() {
            Ok(s) => s,
            Err(e) => {
                let _ = out.send(Event::ConnectError(e.to_string())).await;
                return;
            }
        };

        // Pause registration with CAP LS 302 (some bouncers — soju — NAK
        // any REQ that arrives before LS). Once we see the final LS line
        // we'll build the REQ from the intersection of `wanted` and what
        // the server actually advertised.
        if let Err(e) = sender.send(Command::CAP(
            None,
            CapSubCommand::LS,
            None,
            Some("302".to_string()),
        )) {
            let _ = out.send(Event::ConnectError(e.to_string())).await;
            return;
        }
        let mut auth_phase = AuthPhase::AwaitingCapLs;
        let mut cap_state = CapState::default();
        let mut batches: HashMap<String, BatchInfo> = HashMap::new();

        if use_sasl {
            let mech = match auth_mode {
                AuthMode::SaslExternal => "EXTERNAL",
                _ => "PLAIN",
            };
            let _ = out
                .send(Event::Notice {
                    from: "*".into(),
                    text: format!("authenticating with SASL {mech}…"),
                    meta: MsgMeta::default(),
                })
                .await;
        }

        loop {
            tokio::select! {
                incoming = stream.next() => match incoming {
                    Some(Ok(msg)) => {
                        if auth_phase != AuthPhase::Done {
                            match handle_auth_msg(
                                &msg, &sender, &mut auth_phase, auth_mode,
                                &cfg, &mut cap_state,
                            ) {
                                AuthOutcome::Pending => {}
                                AuthOutcome::NeedIdentify => {
                                    if let Err(e) = client.identify() {
                                        let _ = out
                                            .send(Event::ConnectError(e.to_string()))
                                            .await;
                                        return;
                                    }
                                    auth_phase = AuthPhase::Done;
                                    let acked: Vec<String> =
                                        cap_state.acked.iter().cloned().collect();
                                    let _ = out.send(Event::CapsAcked(acked)).await;
                                    let _ = out.send(Event::Connected).await;
                                }
                                AuthOutcome::Done => {
                                    let acked: Vec<String> =
                                        cap_state.acked.iter().cloned().collect();
                                    let _ = out.send(Event::CapsAcked(acked)).await;
                                    let _ = out.send(Event::Connected).await;
                                }
                                AuthOutcome::Failed(reason) => {
                                    let _ = out.send(Event::ConnectError(reason)).await;
                                    return;
                                }
                            }
                            // Don't surface CAP/AUTHENTICATE/9xx-auth wire chatter to UI.
                            if is_auth_wire(&msg) {
                                continue;
                            }
                        }
                        // Intercept BATCH for netsplit/netjoin grouping; produce
                        // a single summary Notice on close instead of letting
                        // dozens of QUIT/JOIN lines through individually.
                        if let Command::BATCH(ref tag_with_sign, ref sub, ref params) =
                            msg.command
                        {
                            if let Some(id) = tag_with_sign.strip_prefix('+') {
                                let kind = sub
                                    .as_ref()
                                    .map(|s| s.to_str().to_ascii_lowercase())
                                    .unwrap_or_default();
                                batches.insert(
                                    id.to_string(),
                                    BatchInfo {
                                        kind,
                                        params: params.clone().unwrap_or_default(),
                                    },
                                );
                            } else if let Some(id) = tag_with_sign.strip_prefix('-') {
                                if let Some(info) = batches.remove(id) {
                                    if let Some(text) = batch_summary(&info) {
                                        let _ = out
                                            .send(Event::Notice {
                                                from: "*".into(),
                                                text,
                                                meta: MsgMeta::default(),
                                            })
                                            .await;
                                    }
                                }
                            }
                            continue;
                        }
                        for ev in translate(msg, &batches) {
                            if out.send(ev).await.is_err() {
                                return;
                            }
                        }
                    }
                    Some(Err(e)) => {
                        let _ = out.send(Event::ConnectError(e.to_string())).await;
                        return;
                    }
                    None => {
                        let _ = out.send(Event::Disconnected).await;
                        return;
                    }
                },
                outgoing = orx.next() => {
                    match outgoing {
                        Some(Outgoing::Privmsg { target, text }) => {
                            let _ = sender.send_privmsg(&target, &text);
                        }
                        Some(Outgoing::Action { target, text }) => {
                            let wrapped = format!("\x01ACTION {text}\x01");
                            let _ = sender.send_privmsg(&target, &wrapped);
                        }
                        Some(Outgoing::Ctcp { target, query }) => {
                            let wrapped = format!("\x01{query}\x01");
                            let _ = sender.send_privmsg(&target, &wrapped);
                        }
                        Some(Outgoing::Join(channel)) => {
                            let _ = sender.send_join(&channel);
                        }
                        Some(Outgoing::Part { channel, reason }) => {
                            let _ = sender.send(Command::PART(channel, reason));
                        }
                        Some(Outgoing::Nick(new_nick)) => {
                            let _ = sender.send(Command::NICK(new_nick));
                        }
                        Some(Outgoing::ChatHistoryLatest { target, limit }) => {
                            let _ = sender.send(Command::Raw(
                                "CHATHISTORY".into(),
                                vec![
                                    "LATEST".into(),
                                    target,
                                    "*".into(),
                                    limit.to_string(),
                                ],
                            ));
                        }
                        Some(Outgoing::Whois(target)) => {
                            let _ = sender.send(Command::WHOIS(None, target));
                        }
                        Some(Outgoing::Away(msg)) => {
                            let _ = sender.send(Command::AWAY(msg));
                        }
                        Some(Outgoing::Topic { channel, topic }) => {
                            let _ = sender.send(Command::TOPIC(channel, topic));
                        }
                        Some(Outgoing::Raw { cmd, args }) => {
                            let _ = sender.send(Command::Raw(cmd, args));
                        }
                        None => {}
                    }
                }
            }
        }
    })
}

enum AuthOutcome {
    Pending,
    NeedIdentify,
    Done,
    Failed(String),
}

fn handle_auth_msg(
    msg: &Message,
    sender: &Sender,
    phase: &mut AuthPhase,
    mode: AuthMode,
    cfg: &NetworkConfig,
    caps: &mut CapState,
) -> AuthOutcome {
    let use_sasl = matches!(mode, AuthMode::SaslPlain | AuthMode::SaslExternal);
    match &msg.command {
        Command::CAP(_, sub, third, fourth) if *sub == CapSubCommand::LS => {
            if *phase != AuthPhase::AwaitingCapLs {
                return AuthOutcome::Pending;
            }
            // Multi-line LS uses a literal `*` in the slot before the
            // cap list to indicate "more lines follow". We get that as
            // either third == Some("*") or third == None depending on
            // how the parser slots it. Detect by the presence of '*'
            // in `third` while `fourth` holds the list.
            let (more, listed) = match (third.as_deref(), fourth.as_deref()) {
                (Some("*"), Some(list)) => (true, list),
                (Some(list), None) => (false, list),
                (_, Some(list)) => (false, list),
                _ => (false, ""),
            };
            for token in listed.split_whitespace() {
                let name = token
                    .split('=')
                    .next()
                    .unwrap_or(token)
                    .to_ascii_lowercase();
                caps.available.insert(name);
            }
            if more {
                return AuthOutcome::Pending;
            }
            caps.ls_complete = true;

            // Build REQ from the intersection of what we want and what
            // the server offers. SASL piggybacks if configured.
            let mut wanted: Vec<&str> = WANT_EXTRA_CAPS
                .iter()
                .copied()
                .filter(|c| caps.available.contains(*c))
                .collect();
            if use_sasl {
                if !caps.available.contains("sasl") {
                    return AuthOutcome::Failed("server does not support SASL".into());
                }
                wanted.push("sasl");
            }
            if wanted.is_empty() {
                // Nothing to request — close negotiation and identify.
                return AuthOutcome::NeedIdentify;
            }
            let req_str = wanted.join(" ");
            if let Err(e) = sender.send(Command::CAP(
                None,
                CapSubCommand::REQ,
                None,
                Some(req_str),
            )) {
                return AuthOutcome::Failed(format!("send CAP REQ: {e}"));
            }
            *phase = AuthPhase::AwaitingCapAck;
            AuthOutcome::Pending
        }
        Command::CAP(_, sub, third, fourth) if *sub == CapSubCommand::ACK => {
            if *phase != AuthPhase::AwaitingCapAck {
                return AuthOutcome::Pending;
            }
            // Depending on whether the optional second `*` is present, the
            // proto parser drops the trailing cap list into either the 3rd
            // or 4th slot. Check both.
            let listed = fourth
                .as_deref()
                .or(third.as_deref())
                .unwrap_or("");
            for cap in listed.split_whitespace() {
                caps.acked.insert(cap.to_ascii_lowercase());
            }
            if use_sasl {
                if !caps.acked.contains("sasl") {
                    return AuthOutcome::Failed("server ACKed CAP without sasl".into());
                }
                let mech = match mode {
                    AuthMode::SaslExternal => "EXTERNAL",
                    _ => "PLAIN",
                };
                if let Err(e) = sender.send(Command::AUTHENTICATE(mech.to_string())) {
                    return AuthOutcome::Failed(format!("send AUTHENTICATE: {e}"));
                }
                *phase = AuthPhase::AwaitingChallenge;
                AuthOutcome::Pending
            } else {
                AuthOutcome::NeedIdentify
            }
        }
        Command::CAP(_, sub, _, _) if *sub == CapSubCommand::NAK => {
            // Atomic NAK on the combined REQ. For SASL flow this is fatal;
            // for plain registration, just proceed without extras.
            if use_sasl {
                AuthOutcome::Failed("server refused SASL capability".into())
            } else {
                AuthOutcome::NeedIdentify
            }
        }
        Command::AUTHENTICATE(data) if *phase == AuthPhase::AwaitingChallenge => {
            if data != "+" {
                // Server should reply "+" to invite the payload. Anything
                // else here is unexpected; abort cleanly.
                let _ = sender.send(Command::AUTHENTICATE("*".to_string()));
                return AuthOutcome::Failed(format!("unexpected AUTHENTICATE challenge: {data}"));
            }
            let payload = match mode {
                AuthMode::SaslExternal => "+".to_string(),
                _ => {
                    let user = cfg.sasl_user();
                    let pass = cfg.sasl_password.as_deref().unwrap_or("");
                    let raw = build_plain_payload(user, pass);
                    if raw.is_empty() {
                        "+".to_string()
                    } else {
                        b64_encode(&raw)
                    }
                }
            };
            // Per IRCv3: payloads >400 bytes must be split into 400-byte
            // chunks, and an exact-400-multiple needs a trailing "+".
            // Typical credentials sit well under 400, so a single send
            // is enough; if it ever overflows we split.
            for chunk in chunked_400(&payload) {
                if let Err(e) = sender.send(Command::AUTHENTICATE(chunk)) {
                    return AuthOutcome::Failed(format!("send AUTHENTICATE payload: {e}"));
                }
            }
            *phase = AuthPhase::AwaitingResult;
            AuthOutcome::Pending
        }
        Command::Response(code, args) => match *code {
            Response::RPL_SASLSUCCESS if *phase == AuthPhase::AwaitingResult => {
                if let Err(e) = sender.send(Command::CAP(None, CapSubCommand::END, None, None)) {
                    return AuthOutcome::Failed(format!("send CAP END: {e}"));
                }
                if let Err(e) = sender.send(Command::NICK(cfg.nickname.clone())) {
                    return AuthOutcome::Failed(format!("send NICK: {e}"));
                }
                let username = cfg.username.clone().unwrap_or_else(|| cfg.nickname.clone());
                let realname = cfg.realname.clone().unwrap_or_else(|| cfg.nickname.clone());
                if let Err(e) = sender.send(Command::USER(username, "0".into(), realname)) {
                    return AuthOutcome::Failed(format!("send USER: {e}"));
                }
                *phase = AuthPhase::Done;
                AuthOutcome::Done
            }
            Response::ERR_NICKLOCKED
            | Response::ERR_SASLFAIL
            | Response::ERR_SASLTOOLONG
            | Response::ERR_SASLABORT
            | Response::ERR_SASLALREADY => {
                let detail = args
                    .last()
                    .cloned()
                    .unwrap_or_else(|| format!("SASL error {code:?}"));
                AuthOutcome::Failed(format!("SASL failed: {detail}"))
            }
            _ => AuthOutcome::Pending,
        },
        _ => AuthOutcome::Pending,
    }
}

fn is_auth_wire(msg: &Message) -> bool {
    matches!(
        &msg.command,
        Command::CAP(..)
            | Command::AUTHENTICATE(_)
            | Command::Response(
                Response::RPL_LOGGEDIN
                    | Response::RPL_LOGGEDOUT
                    | Response::ERR_NICKLOCKED
                    | Response::RPL_SASLSUCCESS
                    | Response::ERR_SASLFAIL
                    | Response::ERR_SASLTOOLONG
                    | Response::ERR_SASLABORT
                    | Response::ERR_SASLALREADY
                    | Response::RPL_SASLMECHS,
                _,
            )
    )
}

fn build_plain_payload(user: &str, pass: &str) -> Vec<u8> {
    if user.is_empty() && pass.is_empty() {
        return Vec::new();
    }
    let mut v = Vec::with_capacity(user.len() * 2 + pass.len() + 2);
    // SASL PLAIN: authzid \0 authcid \0 password.
    // authzid blank, authcid = user.
    v.push(0);
    v.extend_from_slice(user.as_bytes());
    v.push(0);
    v.extend_from_slice(pass.as_bytes());
    v
}

fn chunked_400(payload: &str) -> Vec<String> {
    if payload.len() < 400 {
        return vec![payload.to_string()];
    }
    let bytes = payload.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let end = (i + 400).min(bytes.len());
        out.push(std::str::from_utf8(&bytes[i..end]).unwrap_or("").to_string());
        i = end;
    }
    if payload.len() % 400 == 0 {
        out.push("+".to_string());
    }
    out
}

fn b64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut chunks = input.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
        out.push(T[((n >> 18) & 0x3f) as usize] as char);
        out.push(T[((n >> 12) & 0x3f) as usize] as char);
        out.push(T[((n >> 6) & 0x3f) as usize] as char);
        out.push(T[(n & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(T[((n >> 18) & 0x3f) as usize] as char);
            out.push(T[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(T[((n >> 18) & 0x3f) as usize] as char);
            out.push(T[((n >> 12) & 0x3f) as usize] as char);
            out.push(T[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

fn extract_meta(
    tags: &Option<Vec<Tag>>,
    batches: &HashMap<String, BatchInfo>,
) -> MsgMeta {
    let mut m = MsgMeta::default();
    let Some(list) = tags.as_ref() else { return m };
    for Tag(k, v) in list {
        match k.as_str() {
            "time" => {
                if let Some(val) = v.as_deref() {
                    m.server_time_hhmm = parse_iso_hhmm(val);
                }
            }
            "msgid" => m.msgid = v.clone(),
            "batch" => {
                m.batch = v.clone();
                if let Some(id) = v.as_deref() {
                    if let Some(info) = batches.get(id) {
                        m.batch_kind = Some(info.kind.clone());
                    }
                }
            }
            _ => {}
        }
    }
    m
}

fn batch_summary(info: &BatchInfo) -> Option<String> {
    let s1 = info.params.first().map(String::as_str).unwrap_or("?");
    let s2 = info.params.get(1).map(String::as_str).unwrap_or("?");
    match info.kind.as_str() {
        "netsplit" => Some(format!("netsplit: {s1} ↮ {s2}")),
        "netjoin" => Some(format!("netjoin: {s1} ↔ {s2}")),
        _ => None,
    }
}

/// Pulls "HH:MM" out of an ISO-8601 timestamp like `2026-05-04T20:30:25.123Z`.
fn parse_iso_hhmm(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() >= 16 && bytes[10] == b'T' {
        Some(s[11..16].to_string())
    } else {
        None
    }
}

fn translate(msg: Message, batches: &HashMap<String, BatchInfo>) -> Vec<Event> {
    let nick = match &msg.prefix {
        Some(Prefix::Nickname(n, _, _)) => n.clone(),
        Some(Prefix::ServerName(s)) => s.clone(),
        None => "*".into(),
    };
    let meta = extract_meta(&msg.tags, batches);
    match msg.command {
        Command::PRIVMSG(target, body) => {
            if let Some(action) = unwrap_ctcp_action(&body) {
                vec![Event::Action {
                    target,
                    nick,
                    body: strip_irc_formatting(&action),
                    meta,
                }]
            } else if is_ctcp_wrapped(&body) {
                // Other CTCP queries (VERSION, PING, TIME, etc.) — auto-handled
                // by the irc crate, suppress the raw echo from the chat view.
                vec![]
            } else {
                vec![Event::Privmsg {
                    target,
                    nick,
                    body: strip_irc_formatting(&body),
                    meta,
                }]
            }
        }
        Command::JOIN(channel, _, _) => vec![Event::UserJoined { channel, nick, meta }],
        Command::PART(channel, _) => vec![Event::UserLeft { channel, nick, meta }],
        Command::NICK(new) => vec![Event::NickChanged { old: nick, new, meta }],
        Command::TOPIC(channel, Some(topic)) => {
            vec![Event::Topic { channel, topic: strip_irc_formatting(&topic) }]
        }
        Command::INVITE(invited, channel) => {
            // With invite-notify, the server forwards INVITEs to channel ops;
            // surface them to &status so we know who got pulled in.
            vec![Event::Notice {
                from: "*".into(),
                text: format!("{nick} invited {invited} to {channel}"),
                meta,
            }]
        }
        Command::NOTICE(_, text) => match unwrap_ctcp_reply(&text) {
            Some((query, args)) => vec![Event::CtcpReply { from: nick, query, args }],
            None => vec![Event::Notice {
                from: nick,
                text: strip_irc_formatting(&text),
                meta,
            }],
        },
        Command::Response(code, args) => match code {
            Response::RPL_NAMREPLY if args.len() >= 4 => {
                let channel = args[2].clone();
                let nicks = args[3]
                    .split_whitespace()
                    .map(strip_prefix)
                    .filter(|n| !n.is_empty())
                    .collect();
                vec![Event::Names { channel, nicks }]
            }
            Response::RPL_TOPIC if args.len() >= 3 => {
                vec![Event::Topic {
                    channel: args[1].clone(),
                    topic: strip_irc_formatting(&args[2]),
                }]
            }
            _ => format_numeric(code, &args)
                .map(|text| vec![Event::Notice { from: "*".into(), text, meta }])
                .unwrap_or_default(),
        },
        // Numerics not in irc-proto's Response enum (330 RPL_WHOISACCOUNT,
        // 338 RPL_WHOISACTUALLY, 671 RPL_WHOISSECURE, etc.) arrive here as
        // Raw("<code>", args).
        Command::Raw(ref cmd, ref args) => {
            if let Ok(n) = cmd.parse::<u16>() {
                if let Some(text) = format_extended_numeric(n, args) {
                    return vec![Event::Notice { from: "*".into(), text, meta }];
                }
            }
            vec![]
        }
        _ => vec![],
    }
}

// Format a known numeric reply into a one-line summary for the status buffer.
// Returns None for numerics we don't surface (everything not enumerated below).
// Args layout follows: args[0] = our nick (client target), args[1..] = payload.
fn format_numeric(code: Response, args: &[String]) -> Option<String> {
    let p = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    match code {
        Response::RPL_AWAY if args.len() >= 3 => {
            Some(format!("{} is away: {}", p(1), p(2)))
        }
        Response::RPL_UNAWAY => Some("you are no longer marked as away".into()),
        Response::RPL_NOWAWAY => Some("you have been marked as away".into()),
        Response::RPL_WHOISUSER if args.len() >= 6 => Some(format!(
            "whois {}: {}!{}@{} — {}",
            p(1), p(1), p(2), p(3), p(5)
        )),
        Response::RPL_WHOISSERVER if args.len() >= 4 => {
            Some(format!("whois {}: server {} ({})", p(1), p(2), p(3)))
        }
        Response::RPL_WHOISOPERATOR if args.len() >= 3 => {
            Some(format!("whois {}: {}", p(1), p(2)))
        }
        Response::RPL_WHOISIDLE if args.len() >= 4 => {
            Some(format!("whois {}: idle {}s, signon {}", p(1), p(2), p(3)))
        }
        Response::RPL_WHOISCHANNELS if args.len() >= 3 => {
            Some(format!("whois {}: channels {}", p(1), p(2)))
        }
        Response::RPL_ENDOFWHOIS if args.len() >= 2 => {
            Some(format!("whois {}: end", p(1)))
        }
        Response::RPL_WHOISCERTFP if args.len() >= 3 => {
            Some(format!("whois {}: {}", p(1), p(2)))
        }
        Response::ERR_NOSUCHNICK if args.len() >= 3 => {
            Some(format!("no such nick: {}", p(1)))
        }
        Response::ERR_NOSUCHCHANNEL if args.len() >= 3 => {
            Some(format!("no such channel: {}", p(1)))
        }
        Response::ERR_CHANOPRIVSNEEDED if args.len() >= 3 => {
            Some(format!("not channel operator: {}", p(1)))
        }
        _ => None,
    }
}

// Format numeric codes that irc-proto doesn't enumerate.
fn format_extended_numeric(code: u16, args: &[String]) -> Option<String> {
    let p = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    match code {
        // 330 RPL_WHOISACCOUNT: <client> <nick> <account> :is logged in as
        330 if args.len() >= 4 => Some(format!("whois {}: account {}", p(1), p(2))),
        // 338 RPL_WHOISACTUALLY: <client> <nick> [...] :Actual host info
        338 if args.len() >= 3 => Some(format!(
            "whois {}: {}",
            p(1),
            args[2..].join(" ")
        )),
        // 671 RPL_WHOISSECURE: <client> <nick> :is using a secure connection
        671 if args.len() >= 3 => Some(format!("whois {}: {}", p(1), p(2))),
        // 378 RPL_WHOISHOST: <client> <nick> :is connecting from ...
        378 if args.len() >= 3 => Some(format!("whois {}: {}", p(1), p(2))),
        // 379 RPL_WHOISMODES: <client> <nick> :is using modes ...
        379 if args.len() >= 3 => Some(format!("whois {}: {}", p(1), p(2))),
        _ => None,
    }
}

fn is_ctcp_wrapped(body: &str) -> bool {
    body.starts_with('\x01') && body.len() >= 2 && body.ends_with('\x01')
}

// Strip mIRC formatting + control codes that we don't render so they
// don't surface as missing-glyph squares in the chat view.
//   \x02 bold   \x1D italic   \x1F underline   \x1E strikethrough
//   \x11 monospace   \x16 reverse   \x0F reset
//   \x03 color: \x03[fg[,bg]] with fg/bg as 1–2 digit decimals
//   \x04 hex color: \x04RRGGBB[,RRGGBB]
fn strip_irc_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x02' | '\x1d' | '\x1f' | '\x1e' | '\x11' | '\x16' | '\x0f' => {}
            '\x03' => {
                let fg = take_digits(&mut chars, 2);
                if fg > 0 {
                    let mut peek = chars.clone();
                    if peek.next() == Some(',')
                        && peek.peek().map(|c| c.is_ascii_digit()).unwrap_or(false)
                    {
                        chars.next();
                        let _ = take_digits(&mut chars, 2);
                    }
                }
            }
            '\x04' => {
                let _ = take_hex(&mut chars, 6);
                let mut peek = chars.clone();
                if peek.next() == Some(',')
                    && peek.peek().map(|c| c.is_ascii_hexdigit()).unwrap_or(false)
                {
                    chars.next();
                    let _ = take_hex(&mut chars, 6);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars>, max: usize) -> usize {
    let mut n = 0;
    while n < max {
        match chars.peek() {
            Some(c) if c.is_ascii_digit() => {
                chars.next();
                n += 1;
            }
            _ => break,
        }
    }
    n
}

fn take_hex(chars: &mut std::iter::Peekable<std::str::Chars>, max: usize) -> usize {
    let mut n = 0;
    while n < max {
        match chars.peek() {
            Some(c) if c.is_ascii_hexdigit() => {
                chars.next();
                n += 1;
            }
            _ => break,
        }
    }
    n
}

fn strip_prefix(n: &str) -> String {
    n.trim_start_matches(['@', '+', '%', '~', '&']).to_string()
}

fn unwrap_ctcp_action(body: &str) -> Option<String> {
    let inner = body.strip_prefix('\x01')?.strip_suffix('\x01')?;
    let rest = inner.strip_prefix("ACTION ")?;
    Some(rest.to_string())
}

fn unwrap_ctcp_reply(text: &str) -> Option<(String, String)> {
    let inner = text.strip_prefix('\x01')?.strip_suffix('\x01')?;
    let mut parts = inner.splitn(2, ' ');
    let q = parts.next()?.trim().to_string();
    if q.is_empty() {
        return None;
    }
    let a = parts.next().unwrap_or("").to_string();
    Some((q, a))
}
