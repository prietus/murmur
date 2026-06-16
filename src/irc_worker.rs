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
    // Tier-1 identity / presence:
    "account-tag",
    "extended-join",
    "account-notify",
    "away-notify",
    "chghost",
    "echo-message",
    "setname",
    // Tier-2 NAMES enrichment:
    "multi-prefix",
    "userhost-in-names",
    // Tier-2 protocol plumbing:
    "cap-notify",
    "labeled-response",
    "sts",
    // Tier-3 drafts:
    "draft/multiline",
    "draft/typing",
    "draft/read-marker",
    "draft/message-redaction",
    "draft/event-playback",
    "draft/sasl-ir",
];

/// Subset of RPL_ISUPPORT (005) tokens that we actually act on. Accumulated
/// across all 005 lines; the App receives the latest snapshot on each update.
#[derive(Clone, Default, Debug)]
pub struct ISupport {
    /// `MODES=<n>` — max mode changes per single MODE command.
    pub modes: Option<u8>,
    /// `CHANTYPES=<chars>` — characters that mark a channel name (`#&`).
    pub chantypes: String,
    /// `PREFIX=(modes)prefixes` — raw mapping (e.g. `(ohv)@%+`).
    pub prefix: String,
    /// `CASEMAPPING=<name>` — `ascii`, `rfc1459`, `rfc1459-strict`, `rfc7613`.
    pub casemapping: String,
    /// `NETWORK=<name>` — human-readable network name.
    pub network: Option<String>,
    /// `soju.im/FILEHOST` / `draft/FILEHOST` — HTTP upload endpoint URI for
    /// the IRCv3 file-upload (FILEHOST) extension. `None` if unadvertised.
    pub filehost: Option<String>,
    /// `MONITOR=<n>` — max number of MONITOR targets we may register.
    /// `Some(u32::MAX)` means advertised with no explicit cap.
    pub monitor_limit: Option<u32>,
    /// `CLIENTTAGDENY=<list>` — server strips these client-only (`+`) tags
    /// from outgoing messages. `*` flips `deny_all` on; `-tag` entries
    /// override into the allow list. See `client_tag_denied()` for lookup.
    pub client_tag_deny_all: bool,
    pub client_tag_deny: std::collections::HashSet<String>,
    pub client_tag_allow: std::collections::HashSet<String>,
}

impl ISupport {
    /// True when the server is advertised to strip the given client-only
    /// tag from outgoing PRIVMSG/TAGMSG. Pass either form: `draft/react`
    /// or `+draft/react` — the leading `+` is normalized.
    pub fn client_tag_denied(&self, tag: &str) -> bool {
        let t = tag.strip_prefix('+').unwrap_or(tag);
        if self.client_tag_allow.contains(t) {
            return false;
        }
        if self.client_tag_deny_all {
            return true;
        }
        self.client_tag_deny.contains(t)
    }
}

/// One entry from a NAMES reply (or a JOIN). Enriched with `multi-prefix`
/// + `userhost-in-names` data when those caps are acked, otherwise just
/// the bare nick.
#[derive(Clone, Debug)]
pub struct MemberEntry {
    pub nick: String,
    /// All channel prefixes for this member, highest-priority first
    /// (e.g. `"@+"`, `"@"`, `"+"`, `""`).
    pub prefixes: String,
    /// `ident@host` without the nick, if `userhost-in-names` is acked.
    pub userhost: Option<String>,
}

/// Per-message metadata extracted from IRCv3 tags.
#[derive(Clone, Default, Debug)]
pub struct MsgMeta {
    /// HH:MM extracted from the `time` tag if present (UTC).
    pub server_time_hhmm: Option<String>,
    /// Full ISO8601 value of the `time` tag, kept verbatim so it can be
    /// used as a `CHATHISTORY BEFORE timestamp=` anchor.
    pub server_time_iso: Option<String>,
    /// Unique server-issued message id from the `msgid` tag.
    pub msgid: Option<String>,
    /// Batch reference tag if this message belongs to an open batch.
    pub batch: Option<String>,
    /// Lower-case batch kind (e.g. "chathistory", "netsplit") looked up
    /// from the open-batch table when the message was processed.
    pub batch_kind: Option<String>,
    /// IRCv3 `account` tag — the sender's services account name when
    /// `account-tag` is negotiated. `None` means logged-out or unsupported.
    pub account: Option<String>,
    /// `+draft/reply=<msgid>` — when present on a PRIVMSG, this message
    /// is threaded as a reply to the message with this msgid.
    pub reply_to_msgid: Option<String>,
}

#[derive(Debug, Clone)]
struct BatchInfo {
    kind: String,
    params: Vec<String>,
    /// For `draft/multiline` batches, the accumulated chunks collected
    /// across PRIVMSGs inside the batch.
    chunks: Vec<MultilineChunk>,
}

#[derive(Debug, Clone)]
struct MultilineChunk {
    target: String,
    nick: String,
    body: String,
    is_action: bool,
    /// True when the chunk carries the `draft/multiline-concat` tag and
    /// should join the previous one without a newline separator.
    concat: bool,
    meta: MsgMeta,
}

#[derive(Clone)]
pub enum Outgoing {
    Privmsg { target: String, text: String },
    /// PRIVMSG carrying a `+draft/reply=<msgid>` tag — threaded reply
    /// to a prior message in the same target.
    PrivmsgReply { target: String, text: String, reply_to_msgid: String },
    Action { target: String, text: String },
    Ctcp { target: String, query: String },
    Join(String),
    Part { channel: String, reason: Option<String> },
    Nick(String),
    /// Fetch the most recent `limit` messages for `target`. Sent as
    /// `CHATHISTORY LATEST <target> * <limit>`.
    ChatHistoryLatest { target: String, limit: u32 },
    /// `CHATHISTORY BEFORE <target> timestamp=<ts> <limit>` — fetch
    /// messages older than the given anchor (scroll-up backfill).
    ChatHistoryBefore { target: String, before_ts: String, limit: u32 },
    /// `CHATHISTORY TARGETS timestamp=<from> timestamp=<to> <limit>` — list
    /// targets with activity in the given window.
    ChatHistoryTargets { from_ts: String, to_ts: String, limit: u32 },
    /// `WHOIS <nick>`. Replies arrive as numerics (311/312/317/318/319/330/671).
    Whois(String),
    /// `AWAY :<msg>` to set, `AWAY` (no arg) to clear.
    Away(Option<String>),
    /// `TOPIC <channel> :<topic>` to set; `topic = None` queries.
    Topic { channel: String, topic: Option<String> },
    /// Raw IRC line. `cmd` and `args` already parsed.
    Raw { cmd: String, args: Vec<String> },
    /// `KICK <channel> <nick> [:reason]`.
    Kick { channel: String, nick: String, reason: Option<String> },
    /// `INVITE <nick> <channel>`.
    Invite { nick: String, channel: String },
    /// `MODE <target> <modes> [args...]`. Sent raw to avoid the typed
    /// `Mode<ChannelMode>` parsing in `irc-proto`.
    Mode { target: String, modes: String, args: Vec<String> },
    /// Send a `draft/typing` TAGMSG to `target` indicating our state.
    Typing { target: String, state: TypingState },
    /// `MARKREAD <target> [timestamp=<iso>]` (draft/read-marker).
    /// `timestamp = None` queries the current marker.
    MarkRead { target: String, timestamp: Option<String> },
    /// `REDACT <target> <msgid> [:reason]` (draft/message-redaction).
    Redact { target: String, msgid: String, reason: Option<String> },
    /// Send a `+draft/reply=<msgid>` TAGMSG carrying an emoji reaction.
    React { target: String, msgid: String, emoji: String },
    /// `SETNAME :<new realname>` (IRCv3 setname cap). Changes realname
    /// mid-session without reconnecting.
    SetName(String),
    /// `MONITOR +/-/C/L/S` — manage the server-side presence subscription
    /// list. Server replies arrive as RPL_MONONLINE/OFFLINE.
    Monitor(MonitorCmd),
}

#[derive(Clone, Debug)]
pub enum MonitorCmd {
    /// `MONITOR + nick[,nick,...]` — add targets.
    Add(Vec<String>),
    /// `MONITOR - nick[,nick,...]` — remove targets.
    Del(Vec<String>),
    /// `MONITOR C` — clear all server-side targets.
    Clear,
}

#[derive(Clone, Copy, Debug)]
pub enum TypingState {
    Active,
    Paused,
    Done,
}

impl TypingState {
    fn as_str(self) -> &'static str {
        match self {
            TypingState::Active => "active",
            TypingState::Paused => "paused",
            TypingState::Done => "done",
        }
    }
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
    /// Recoverable disconnect; another attempt is scheduled in `in_secs`.
    /// The UI uses this to keep the dot orange (Connecting) instead of red.
    Reconnecting { in_secs: u64 },
    Privmsg { target: String, nick: String, body: String, meta: MsgMeta },
    Action { target: String, nick: String, body: String, meta: MsgMeta },
    UserJoined {
        channel: String,
        nick: String,
        /// `ident@host` from the message prefix when present.
        userhost: Option<String>,
        /// Services account from `extended-join` (None means logged-out/`*`).
        account: Option<String>,
        /// Realname from `extended-join`.
        realname: Option<String>,
        meta: MsgMeta,
    },
    UserLeft { channel: String, nick: String, meta: MsgMeta },
    /// QUIT: a user disconnected from the network. The UI is responsible
    /// for removing the nick from every channel's member list on this
    /// network.
    UserQuit { nick: String, reason: Option<String>, meta: MsgMeta },
    /// A `chathistory` batch closed. `target` is the channel/nick the
    /// batch was for; lets the UI clear its history-loading state.
    ChatHistoryBatchEnd { target: String },
    NickChanged { old: String, new: String, meta: MsgMeta },
    Names { channel: String, members: Vec<MemberEntry> },
    Topic { channel: String, topic: String },
    Notice { from: String, text: String, meta: MsgMeta },
    CtcpReply { from: String, query: String, args: String },
    /// `account-notify`: a user logged in/out of services.
    AccountChanged { nick: String, account: Option<String>, meta: MsgMeta },
    /// `away-notify`: a user marked themselves away or returned.
    AwayChanged { nick: String, message: Option<String>, meta: MsgMeta },
    /// `chghost`: a user's ident/host changed in place.
    HostChanged { nick: String, ident: String, host: String, meta: MsgMeta },
    /// `RPL_ISUPPORT` (005) accumulated snapshot — sent each time the
    /// server announces new tokens.
    ISupport(ISupport),
    /// `draft/typing` TAGMSG from another user.
    TypingChanged { target: String, nick: String, state: TypingState },
    /// `draft/read-marker` MARKREAD reply or push.
    ReadMarker { target: String, timestamp: Option<String> },
    /// `draft/message-redaction` REDACT — a message was deleted.
    Redacted { target: String, msgid: String, by_nick: String, reason: Option<String> },
    /// Inbound emoji reaction (TAGMSG with `+draft/reply` and a body emoji).
    Reaction { target: String, target_msgid: String, nick: String, emoji: String },
    /// `RPL_MONONLINE` (730) / `RPL_MONOFFLINE` (731) — one or more
    /// monitored nicks have come online or gone offline.
    Presence { nicks: Vec<String>, online: bool },
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
    /// Caps the server announced via CAP LS.
    available: HashSet<String>,
    /// Map of cap name → value for caps that came as `name=value`
    /// (used by `sts`, `draft/multiline`, etc.).
    values: HashMap<String, String>,
    /// Whether we've seen the final (non-multiline) CAP LS response.
    ls_complete: bool,
    /// Caps the server actually ACKed for this session.
    acked: HashSet<String>,
}

pub fn subscribe(cfg: &NetworkConfig) -> impl Stream<Item = Event> + Send + 'static {
    let mut cfg = cfg.clone();
    // Apply a persisted IRCv3 STS policy if there is one. The policy says
    // "future connections to this host must use TLS on this port until X".
    let sts_applied = if let Some(policy) = crate::config::sts::get_active(&cfg.server) {
        let mut changed = false;
        if !cfg.use_tls {
            cfg.use_tls = true;
            changed = true;
        }
        if cfg.port != policy.port {
            cfg.port = policy.port;
            changed = true;
        }
        changed.then_some(policy.port)
    } else {
        None
    };
    iced::stream::channel(128, move |mut out: mpsc::Sender<Event>| async move {
        let (otx, mut orx) = mpsc::channel::<Outgoing>(64);
        if out.send(Event::Ready(otx)).await.is_err() {
            return;
        }

        if let Some(port) = sts_applied {
            let _ = out
                .send(Event::Notice {
                    from: "*".into(),
                    text: format!(
                        "STS policy active: forcing TLS on port {port} for {}",
                        cfg.server
                    ),
                    meta: MsgMeta::default(),
                })
                .await;
        }

        let auth_mode = cfg.auth_mode();
        let use_sasl = matches!(auth_mode, AuthMode::SaslPlain | AuthMode::SaslExternal);

        // Reconnect loop. Each iteration is one connection attempt. On a
        // recoverable failure (TCP drop, PingTimeout, server-closed stream)
        // we back off and try again; the same `otx`/`orx` pair is reused
        // across attempts so the App's stored Outgoing sender stays valid.
        let mut attempt: u32 = 0;
        loop {
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
                // CTCP auto-replies sent by the irc crate. Identify as
                // murmur instead of the crate's default "irc:VERSION:env".
                version: Some(format!(
                    "murmur {} (iced) — https://murmur.priet.us",
                    env!("CARGO_PKG_VERSION")
                )),
                source: Some("https://murmur.priet.us".into()),
                ..Config::default()
            };

            let outcome: AttemptOutcome = 'attempt: {
                let mut client = match Client::from_config(irc_cfg).await {
                    Ok(c) => c,
                    Err(e) => {
                        break 'attempt AttemptOutcome::Recoverable(e.to_string());
                    }
                };

                let sender = client.sender();
                let mut stream = match client.stream() {
                    Ok(s) => s,
                    Err(e) => {
                        break 'attempt AttemptOutcome::Fatal(e.to_string());
                    }
                };

                // Pause registration with CAP LS 302 (some bouncers — soju —
                // NAK any REQ that arrives before LS). Once we see the final
                // LS line we'll build the REQ from the intersection of
                // `wanted` and what the server actually advertised.
                if let Err(e) = sender.send(Command::CAP(
                    None,
                    CapSubCommand::LS,
                    None,
                    Some("302".to_string()),
                )) {
                    break 'attempt AttemptOutcome::Recoverable(e.to_string());
                }
                let mut auth_phase = AuthPhase::AwaitingCapLs;
                let mut cap_state = CapState::default();
                let mut batches: HashMap<String, BatchInfo> = HashMap::new();
                let mut isupport = ISupport::default();

                loop {
                    tokio::select! {
                        incoming = stream.next() => match incoming {
                            Some(Ok(msg)) => {
                                if auth_phase == AuthPhase::Done {
                                    // `cap-notify` push: handle CAP NEW/ACK/DEL
                                    // mid-session (typically from bouncers).
                                    if let Some(updated) =
                                        handle_cap_notify(&msg, &sender, &mut cap_state)
                                    {
                                        let _ = out.send(Event::CapsAcked(updated)).await;
                                    }
                                    // Drop CAP wire chatter from the chat view.
                                    if matches!(&msg.command, Command::CAP(..)) {
                                        continue;
                                    }
                                }
                                if auth_phase != AuthPhase::Done {
                                    match handle_auth_msg(
                                        &msg, &sender, &mut auth_phase, auth_mode,
                                        &cfg, &mut cap_state,
                                    ) {
                                        AuthOutcome::Pending => {}
                                        AuthOutcome::NeedIdentify => {
                                            if let Err(e) = client.identify() {
                                                break 'attempt
                                                    AttemptOutcome::Fatal(e.to_string());
                                            }
                                            auth_phase = AuthPhase::Done;
                                            let acked: Vec<String> =
                                                cap_state.acked.iter().cloned().collect();
                                            let _ = out.send(Event::CapsAcked(acked)).await;
                                            let _ = out.send(Event::Connected).await;
                                            attempt = 0;
                                        }
                                        AuthOutcome::Done => {
                                            let acked: Vec<String> =
                                                cap_state.acked.iter().cloned().collect();
                                            let _ = out.send(Event::CapsAcked(acked)).await;
                                            // Brief positive confirmation in
                                            // &status — helpful for cert/auth
                                            // troubleshooting.
                                            let mech = match auth_mode {
                                                AuthMode::SaslExternal => "EXTERNAL",
                                                AuthMode::SaslPlain => "PLAIN",
                                                _ => "",
                                            };
                                            if !mech.is_empty() {
                                                let _ = out
                                                    .send(Event::Notice {
                                                        from: "*".into(),
                                                        text: format!(
                                                            "SASL {mech} authentication successful"
                                                        ),
                                                        meta: MsgMeta::default(),
                                                    })
                                                    .await;
                                            }
                                            let _ = out.send(Event::Connected).await;
                                            attempt = 0;
                                        }
                                        AuthOutcome::Failed(reason) => {
                                            break 'attempt AttemptOutcome::Fatal(reason);
                                        }
                                    }
                                    // Don't surface CAP/AUTHENTICATE/9xx-auth wire chatter to UI.
                                    if is_auth_wire(&msg) {
                                        continue;
                                    }
                                }
                                // Intercept BATCH for netsplit/netjoin grouping;
                                // produce a single summary Notice on close
                                // instead of letting dozens of QUIT/JOIN
                                // lines through individually.
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
                                                chunks: Vec::new(),
                                            },
                                        );
                                    } else if let Some(id) = tag_with_sign.strip_prefix('-') {
                                        if let Some(info) = batches.remove(id) {
                                            if info.kind == "chathistory" {
                                                if let Some(target) = info.params.first() {
                                                    let _ = out
                                                        .send(Event::ChatHistoryBatchEnd {
                                                            target: target.clone(),
                                                        })
                                                        .await;
                                                }
                                            } else if let Some(ev) = finalize_multiline(&info) {
                                                if out.send(ev).await.is_err() {
                                                    return;
                                                }
                                            } else if let Some(text) = batch_summary(&info) {
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
                                // If this message belongs to an open
                                // draft/multiline batch, accumulate it and
                                // don't emit a per-chunk event — the batch
                                // close will produce one combined.
                                if accumulate_multiline_chunk(&msg, &mut batches) {
                                    continue;
                                }
                                for ev in translate(msg, &batches, &mut isupport) {
                                    if out.send(ev).await.is_err() {
                                        return;
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                break 'attempt AttemptOutcome::Recoverable(e.to_string());
                            }
                            None => {
                                break 'attempt AttemptOutcome::Recoverable(
                                    "server closed connection".into(),
                                );
                            }
                        },
                        outgoing = orx.next() => {
                            match outgoing {
                                Some(Outgoing::Privmsg { target, text }) => {
                                    let _ = sender.send_privmsg(&target, &text);
                                }
                                Some(Outgoing::PrivmsgReply { target, text, reply_to_msgid }) => {
                                    let _ = sender.send(Message {
                                        tags: Some(vec![Tag(
                                            "+draft/reply".into(),
                                            Some(reply_to_msgid),
                                        )]),
                                        prefix: None,
                                        command: Command::PRIVMSG(target, text),
                                    });
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
                                Some(Outgoing::ChatHistoryBefore { target, before_ts, limit }) => {
                                    let _ = sender.send(Command::Raw(
                                        "CHATHISTORY".into(),
                                        vec![
                                            "BEFORE".into(),
                                            target,
                                            format!("timestamp={before_ts}"),
                                            limit.to_string(),
                                        ],
                                    ));
                                }
                                Some(Outgoing::ChatHistoryTargets { from_ts, to_ts, limit }) => {
                                    let _ = sender.send(Command::Raw(
                                        "CHATHISTORY".into(),
                                        vec![
                                            "TARGETS".into(),
                                            format!("timestamp={from_ts}"),
                                            format!("timestamp={to_ts}"),
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
                                Some(Outgoing::Kick { channel, nick, reason }) => {
                                    let _ = sender.send(Command::KICK(channel, nick, reason));
                                }
                                Some(Outgoing::Invite { nick, channel }) => {
                                    let _ = sender.send(Command::INVITE(nick, channel));
                                }
                                Some(Outgoing::Mode { target, modes, args }) => {
                                    let mut argv = Vec::with_capacity(2 + args.len());
                                    argv.push(target);
                                    argv.push(modes);
                                    argv.extend(args);
                                    let _ = sender.send(Command::Raw("MODE".into(), argv));
                                }
                                Some(Outgoing::Typing { target, state }) => {
                                    let _ = sender.send(Message {
                                        tags: Some(vec![Tag(
                                            "+typing".into(),
                                            Some(state.as_str().into()),
                                        )]),
                                        prefix: None,
                                        command: Command::Raw("TAGMSG".into(), vec![target]),
                                    });
                                }
                                Some(Outgoing::MarkRead { target, timestamp }) => {
                                    let mut argv = vec![target];
                                    if let Some(ts) = timestamp {
                                        argv.push(format!("timestamp={ts}"));
                                    }
                                    let _ = sender.send(Command::Raw("MARKREAD".into(), argv));
                                }
                                Some(Outgoing::Redact { target, msgid, reason }) => {
                                    let mut argv = vec![target, msgid];
                                    if let Some(r) = reason {
                                        argv.push(r);
                                    }
                                    let _ = sender.send(Command::Raw("REDACT".into(), argv));
                                }
                                Some(Outgoing::SetName(realname)) => {
                                    let _ = sender.send(Command::Raw(
                                        "SETNAME".into(),
                                        vec![realname],
                                    ));
                                }
                                Some(Outgoing::Monitor(cmd)) => {
                                    let (op, payload) = match cmd {
                                        MonitorCmd::Add(t) => ("+", Some(t.join(","))),
                                        MonitorCmd::Del(t) => ("-", Some(t.join(","))),
                                        MonitorCmd::Clear => ("C", None),
                                    };
                                    // Build argv unless payload is an empty
                                    // add/del — those are no-ops to skip.
                                    let argv = match payload {
                                        Some(p) if p.is_empty() => None,
                                        Some(p) => Some(vec![op.to_string(), p]),
                                        None => Some(vec![op.to_string()]),
                                    };
                                    if let Some(argv) = argv {
                                        let _ = sender.send(Command::Raw(
                                            "MONITOR".into(),
                                            argv,
                                        ));
                                    }
                                }
                                Some(Outgoing::React { target, msgid, emoji }) => {
                                    // Reactions piggyback on TAGMSG with
                                    // +draft/reply pointing at the target
                                    // msgid; body lives in +draft/react.
                                    let _ = sender.send(Message {
                                        tags: Some(vec![
                                            Tag("+draft/reply".into(), Some(msgid)),
                                            Tag("+draft/react".into(), Some(emoji)),
                                        ]),
                                        prefix: None,
                                        command: Command::Raw("TAGMSG".into(), vec![target]),
                                    });
                                }
                                None => {}
                            }
                        }
                    }
                }
            };

            match outcome {
                AttemptOutcome::Fatal(e) => {
                    let _ = out.send(Event::ConnectError(e)).await;
                    return;
                }
                AttemptOutcome::Recoverable(e) => {
                    let secs = backoff_secs(attempt);
                    attempt = attempt.saturating_add(1);
                    let _ = out
                        .send(Event::Notice {
                            from: "*".into(),
                            text: format!(
                                "disconnected: {e} — reconnecting in {secs}s"
                            ),
                            meta: MsgMeta::default(),
                        })
                        .await;
                    let _ = out.send(Event::Reconnecting { in_secs: secs }).await;
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
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

/// Result of one connection attempt in the reconnect loop.
enum AttemptOutcome {
    /// Network-level failure — retry with backoff (TCP drop, PingTimeout,
    /// server-closed stream, transient connect error).
    Recoverable(String),
    /// Won't get better by retrying (SASL credentials rejected, malformed
    /// config the client crate refused, etc.). Stop and stay red.
    Fatal(String),
}

/// Backoff schedule: 2, 4, 8, 16, 30, then 60 cap.
fn backoff_secs(attempt: u32) -> u64 {
    match attempt {
        0 => 2,
        1 => 4,
        2 => 8,
        3 => 16,
        4 => 30,
        _ => 60,
    }
}

// Once auth is complete, the server may push CAP NEW / CAP DEL via the
// `cap-notify` capability (soju does this when a bouncer network attaches
// or detaches, exposing or hiding its caps). We auto-REQ any newly-offered
// caps in our wanted set and track post-auth ACKs / DELs in `caps.acked`.
//
// Returns `Some(snapshot)` of the updated acked set if it changed, so the
// caller can push an updated `Event::CapsAcked` to the App.
fn handle_cap_notify(
    msg: &Message,
    sender: &Sender,
    caps: &mut CapState,
) -> Option<Vec<String>> {
    let Command::CAP(_, sub, third, fourth) = &msg.command else {
        return None;
    };
    let listed = fourth.as_deref().or(third.as_deref()).unwrap_or("");
    match *sub {
        CapSubCommand::NEW => {
            let mut to_req: Vec<String> = Vec::new();
            for token in listed.split_whitespace() {
                let (name_raw, value) = match token.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (token, None),
                };
                let name = name_raw.to_ascii_lowercase();
                caps.available.insert(name.clone());
                if let Some(v) = value {
                    caps.values.insert(name.clone(), v);
                }
                if WANT_EXTRA_CAPS.contains(&name.as_str()) && !caps.acked.contains(&name) {
                    to_req.push(name);
                }
            }
            if !to_req.is_empty() {
                let _ = sender.send(Command::CAP(
                    None,
                    CapSubCommand::REQ,
                    None,
                    Some(to_req.join(" ")),
                ));
            }
            None
        }
        CapSubCommand::ACK => {
            let mut changed = false;
            for cap in listed.split_whitespace() {
                if caps.acked.insert(cap.to_ascii_lowercase()) {
                    changed = true;
                }
            }
            changed.then(|| caps.acked.iter().cloned().collect())
        }
        CapSubCommand::DEL => {
            let mut changed = false;
            for cap in listed.split_whitespace() {
                let lower = cap.to_ascii_lowercase();
                caps.available.remove(&lower);
                caps.values.remove(&lower);
                if caps.acked.remove(&lower) {
                    changed = true;
                }
            }
            changed.then(|| caps.acked.iter().cloned().collect())
        }
        _ => None,
    }
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
                let (name_raw, value) = match token.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_string())),
                    None => (token, None),
                };
                let name = name_raw.to_ascii_lowercase();
                caps.available.insert(name.clone());
                if let Some(v) = value {
                    caps.values.insert(name, v);
                }
            }
            if more {
                return AuthOutcome::Pending;
            }
            caps.ls_complete = true;

            // STS — if the server announced an `sts=duration=N,port=P` value
            // and we're already on TLS, persist the policy so the next
            // connection attempt forces the right port + TLS.
            if let Some(value) = caps.values.get("sts").cloned() {
                if let Some((port, duration)) = parse_sts_value(&value) {
                    if cfg.use_tls && duration > 0 {
                        let _ = crate::config::sts::upsert(&cfg.server, port, duration);
                    }
                }
            }

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
            // Send NICK + USER before AUTHENTICATE. The IRCv3 SASL spec
            // allows registration commands at any point during cap
            // negotiation, but some servers (notably soju) refuse the
            // SASL exchange until NICK is on file ("Expected NICK
            // command before AUTHENTICATE"). Send pre-emptively when we
            // intend to do SASL.
            if use_sasl {
                if let Err(e) = sender.send(Command::NICK(cfg.nickname.clone())) {
                    return AuthOutcome::Failed(format!("send NICK: {e}"));
                }
                let username = cfg
                    .username
                    .clone()
                    .unwrap_or_else(|| cfg.nickname.clone());
                let realname = cfg
                    .realname
                    .clone()
                    .unwrap_or_else(|| cfg.nickname.clone());
                if let Err(e) =
                    sender.send(Command::USER(username, "0".into(), realname))
                {
                    return AuthOutcome::Failed(format!("send USER: {e}"));
                }
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
                // With `draft/sasl-ir` we can attach the initial response on
                // the first AUTHENTICATE line, skipping the server's `+`
                // challenge entirely. Fall back to the classic two-step
                // flow if the payload would exceed one line (>400 bytes)
                // since the IR variant can't be chunked.
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
                let use_ir =
                    caps.acked.contains("draft/sasl-ir") && payload.len() < 400;
                if use_ir {
                    if let Err(e) = sender.send(Command::Raw(
                        "AUTHENTICATE".into(),
                        vec![mech.to_string(), payload],
                    )) {
                        return AuthOutcome::Failed(format!("send AUTHENTICATE: {e}"));
                    }
                    *phase = AuthPhase::AwaitingResult;
                } else {
                    if let Err(e) = sender.send(Command::AUTHENTICATE(mech.to_string())) {
                        return AuthOutcome::Failed(format!("send AUTHENTICATE: {e}"));
                    }
                    *phase = AuthPhase::AwaitingChallenge;
                }
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
                // NICK + USER were already sent before the CAP REQ to
                // satisfy strict servers like soju. Just close cap
                // negotiation here.
                if let Err(e) = sender.send(Command::CAP(None, CapSubCommand::END, None, None)) {
                    return AuthOutcome::Failed(format!("send CAP END: {e}"));
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

// Parse the IRCv3 `sts=` cap value into `(port, duration_secs)`.
// Format: comma-separated `key=value` pairs; `port` and `duration` are
// the two we care about. Returns None if `port` is missing.
fn parse_sts_value(s: &str) -> Option<(u16, u64)> {
    let mut port: Option<u16> = None;
    let mut duration: u64 = 0;
    for part in s.split(',') {
        let (k, v) = match part.split_once('=') {
            Some(kv) => kv,
            None => (part, ""),
        };
        match k.trim() {
            "port" => port = v.trim().parse().ok(),
            "duration" => duration = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    port.map(|p| (p, duration))
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
                    m.server_time_iso = Some(val.to_string());
                }
            }
            // Standard IRCv3 `msgid`. `draft/msgid` is the pre-stabilisation
            // name some older servers still emit. Only overwrite from the
            // draft form if we haven't already seen the standard one.
            "msgid" => m.msgid = v.clone(),
            "draft/msgid" => {
                if m.msgid.is_none() {
                    m.msgid = v.clone();
                }
            }
            "account" => {
                // `account=*` means logged-out per IRCv3; treat as None.
                m.account = v.clone().filter(|s| !s.is_empty() && s != "*");
            }
            "batch" => {
                m.batch = v.clone();
                if let Some(id) = v.as_deref() {
                    if let Some(info) = batches.get(id) {
                        m.batch_kind = Some(info.kind.clone());
                    }
                }
            }
            // `+draft/reply` also rides on TAGMSG to carry reactions;
            // the TAGMSG path handles that case separately, so this
            // assignment is harmless there (PRIVMSG/Action are what
            // actually consume `reply_to_msgid` downstream).
            "+draft/reply" => {
                m.reply_to_msgid = v.clone().filter(|s| !s.is_empty());
            }
            _ => {}
        }
    }
    m
}

// Returns true if the message was a PRIVMSG inside an open multiline batch
// (and was therefore absorbed). Caller should skip further processing.
fn accumulate_multiline_chunk(
    msg: &Message,
    batches: &mut HashMap<String, BatchInfo>,
) -> bool {
    let Command::PRIVMSG(target, body) = &msg.command else { return false };
    let Some(tags) = &msg.tags else { return false };
    // Find batch tag.
    let mut batch_id: Option<&str> = None;
    let mut concat = false;
    for Tag(k, v) in tags {
        match k.as_str() {
            "batch" => batch_id = v.as_deref(),
            "draft/multiline-concat" => concat = true,
            _ => {}
        }
    }
    let Some(id) = batch_id else { return false };
    // Compute everything that needs `&batches` before taking the mut borrow.
    if batches.get(id).map(|i| i.kind.as_str()) != Some("draft/multiline") {
        return false;
    }
    let chunk_meta = extract_meta(&msg.tags, batches);
    let nick = match &msg.prefix {
        Some(Prefix::Nickname(n, _, _)) => n.clone(),
        Some(Prefix::ServerName(s)) => s.clone(),
        None => "*".into(),
    };
    // Detect /me-style chunks so we surface them as Action when all chunks
    // are actions (a single multiline message can't mix kinds in practice).
    let (clean_body, is_action) = match unwrap_ctcp_action(body) {
        Some(action) => (action, true),
        None => (body.clone(), false),
    };
    let Some(info) = batches.get_mut(id) else { return false };
    info.chunks.push(MultilineChunk {
        target: target.clone(),
        nick,
        body: strip_irc_formatting(&clean_body),
        is_action,
        concat,
        meta: chunk_meta,
    });
    true
}

// On batch close, fold accumulated multiline chunks into a single event.
fn finalize_multiline(info: &BatchInfo) -> Option<Event> {
    if info.kind != "draft/multiline" || info.chunks.is_empty() {
        return None;
    }
    let mut body = String::new();
    for (i, chunk) in info.chunks.iter().enumerate() {
        if i > 0 && !chunk.concat {
            body.push('\n');
        }
        body.push_str(&chunk.body);
    }
    let first = &info.chunks[0];
    let is_action = info.chunks.iter().all(|c| c.is_action);
    let target = first.target.clone();
    let nick = first.nick.clone();
    let meta = first.meta.clone();
    if is_action {
        Some(Event::Action { target, nick, body, meta })
    } else {
        Some(Event::Privmsg { target, nick, body, meta })
    }
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

/// Pulls "HH:MM" out of an ISO-8601 UTC timestamp like
/// `2026-05-04T20:30:25.123Z` and shifts it into the local zone using the
/// offset captured at process start. `server-time` is mandated UTC by IRCv3.
fn parse_iso_hhmm(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.len() >= 16 && bytes[10] == b'T' {
        let h: i64 = s[11..13].parse().ok()?;
        let m: i64 = s[14..16].parse().ok()?;
        let total = (h * 60 + m + crate::local_offset_secs() / 60).rem_euclid(1440);
        let h = total / 60;
        let m = total % 60;
        Some(format!("{h:02}:{m:02}"))
    } else {
        None
    }
}

fn translate(
    msg: Message,
    batches: &HashMap<String, BatchInfo>,
    isupport: &mut ISupport,
) -> Vec<Event> {
    let (nick, sender_userhost) = match &msg.prefix {
        Some(Prefix::Nickname(n, ident, host)) => {
            let uh = if !ident.is_empty() && !host.is_empty() {
                Some(format!("{ident}@{host}"))
            } else {
                None
            };
            (n.clone(), uh)
        }
        Some(Prefix::ServerName(s)) => (s.clone(), None),
        None => ("*".into(), None),
    };
    let meta = extract_meta(&msg.tags, batches);
    // Snapshot the raw tags before the `msg.command` move; TAGMSG handlers
    // need them after we've matched on command.
    let msg_tags_raw: Vec<(String, Option<String>)> = msg
        .tags
        .as_ref()
        .map(|ts| ts.iter().map(|Tag(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
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
        Command::JOIN(channel, account, realname) => {
            // With `extended-join`, JOIN carries `<account> :<realname>`.
            // `*` (or empty) means the user is not logged in to services.
            let account = account.filter(|s| !s.is_empty() && s != "*");
            let realname = realname.filter(|s| !s.is_empty());
            vec![Event::UserJoined {
                channel,
                nick,
                userhost: sender_userhost,
                account,
                realname,
                meta,
            }]
        }
        Command::PART(channel, _) => vec![Event::UserLeft { channel, nick, meta }],
        Command::QUIT(reason) => {
            let reason = reason.filter(|s| !s.is_empty());
            vec![Event::UserQuit { nick, reason, meta }]
        }
        Command::NICK(new) => vec![Event::NickChanged { old: nick, new, meta }],
        Command::ACCOUNT(account) => {
            // `account-notify`: a remote user's services-login state changed.
            let account = if account == "*" || account.is_empty() {
                None
            } else {
                Some(account)
            };
            vec![Event::AccountChanged { nick, account, meta }]
        }
        Command::AWAY(message) => {
            // `away-notify`: other users send AWAY when they change state.
            // Our own AWAY-set reflects back as 305/306 numerics instead.
            let message = message.filter(|s| !s.is_empty());
            vec![Event::AwayChanged { nick, message, meta }]
        }
        Command::CHGHOST(ident, host) => {
            vec![Event::HostChanged { nick, ident, host, meta }]
        }
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
        Command::KICK(channel, target, reason) => {
            let text = match reason {
                Some(r) if !r.is_empty() => {
                    format!("{nick} kicked {target} from {channel} ({r})")
                }
                _ => format!("{nick} kicked {target} from {channel}"),
            };
            vec![
                Event::UserLeft { channel, nick: target, meta: meta.clone() },
                Event::Notice { from: "*".into(), text, meta },
            ]
        }
        Command::ChannelMODE(channel, modes) => {
            let rendered = render_modes(&modes);
            vec![Event::Notice {
                from: "*".into(),
                text: format!("{nick} sets mode {channel} {rendered}"),
                meta,
            }]
        }
        Command::UserMODE(target, modes) => {
            let rendered = render_modes(&modes);
            vec![Event::Notice {
                from: "*".into(),
                text: format!("{nick} sets mode {target} {rendered}"),
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
            Response::RPL_ISUPPORT => {
                // Skip args[0] (our nick) and the trailing
                // ":are supported by this server".
                let token_count = args.len().saturating_sub(1);
                let mut changed = false;
                for tok in args.iter().skip(1).take(token_count.saturating_sub(1)) {
                    if apply_isupport_token(isupport, tok) {
                        changed = true;
                    }
                }
                if changed {
                    vec![Event::ISupport(isupport.clone())]
                } else {
                    vec![]
                }
            }
            Response::RPL_NAMREPLY if args.len() >= 4 => {
                let channel = args[2].clone();
                let members = args[3]
                    .split_whitespace()
                    .filter_map(parse_name_entry)
                    .collect();
                vec![Event::Names { channel, members }]
            }
            Response::RPL_TOPIC if args.len() >= 3 => {
                vec![Event::Topic {
                    channel: args[1].clone(),
                    topic: strip_irc_formatting(&args[2]),
                }]
            }
            Response::RPL_MONONLINE if args.len() >= 2 => {
                let nicks = parse_monitor_targets(&args[1]);
                if nicks.is_empty() {
                    vec![]
                } else {
                    vec![Event::Presence { nicks, online: true }]
                }
            }
            Response::RPL_MONOFFLINE if args.len() >= 2 => {
                let nicks = parse_monitor_targets(&args[1]);
                if nicks.is_empty() {
                    vec![]
                } else {
                    vec![Event::Presence { nicks, online: false }]
                }
            }
            // 732 RPL_MONLIST and 733 RPL_ENDOFMONLIST: swallowed in v1
            // (no UI consumer). 734 ERR_MONLISTFULL is surfaced as Notice
            // by the catch-all arm so the user sees the limit.
            Response::RPL_MONLIST | Response::RPL_ENDOFMONLIST => vec![],
            _ => format_numeric(code, &args)
                .or_else(|| render_raw_numeric(code as u16, &args))
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
                if let Some(text) = render_raw_numeric(n, args) {
                    return vec![Event::Notice { from: "*".into(), text, meta }];
                }
            }
            // IRCv3 standard replies: FAIL/WARN/NOTE <command> <code> [context]* :description
            if let Some(text) = format_standard_reply(cmd, args) {
                return vec![Event::Notice { from: "*".into(), text, meta }];
            }
            // CHATHISTORY TARGETS reply: one line per target.
            //   :server CHATHISTORY TARGETS <target> <timestamp>
            if cmd.eq_ignore_ascii_case("CHATHISTORY")
                && args.first().map(|s| s.eq_ignore_ascii_case("TARGETS")).unwrap_or(false)
                && args.len() >= 3
            {
                return vec![Event::Notice {
                    from: "*".into(),
                    text: format!("history target: {} @ {}", args[1], args[2]),
                    meta,
                }];
            }
            // TAGMSG carries no body — the meaning lives in IRCv3 tags
            // (`+typing`, `+draft/react`+`+draft/reply`, etc.).
            if cmd.eq_ignore_ascii_case("TAGMSG") && !args.is_empty() {
                if let Some(events) = parse_tagmsg_event(&nick, &args[0], &msg_tags_raw) {
                    return events;
                }
                return vec![];
            }
            // MARKREAD: `:server MARKREAD <target> [timestamp=<iso>]`
            if cmd.eq_ignore_ascii_case("MARKREAD") && !args.is_empty() {
                let target = args[0].clone();
                let timestamp = args.iter()
                    .skip(1)
                    .find_map(|s| s.strip_prefix("timestamp=").map(str::to_string));
                return vec![Event::ReadMarker { target, timestamp }];
            }
            // SETNAME: `:nick!user@host SETNAME :<new realname>`. Surface
            // realname changes as a one-line notice in &status.
            if cmd.eq_ignore_ascii_case("SETNAME") && !args.is_empty() {
                let new = args.last().cloned().unwrap_or_default();
                return vec![Event::Notice {
                    from: "*".into(),
                    text: format!("{nick} updated realname → {new}"),
                    meta,
                }];
            }
            // REDACT: `:nick REDACT <target> <msgid> [:reason]`
            if cmd.eq_ignore_ascii_case("REDACT") && args.len() >= 2 {
                let target = args[0].clone();
                let msgid = args[1].clone();
                let reason = args.get(2).cloned();
                return vec![Event::Redacted {
                    target,
                    msgid,
                    by_nick: nick.clone(),
                    reason,
                }];
            }
            vec![]
        }
        _ => vec![],
    }
}

// Format a known numeric reply into a one-line summary for the status buffer.
// Returns None for numerics we don't surface (everything not enumerated below).
// Args layout follows: args[0] = our nick (client target), args[1..] = payload.
// Pretty-print a vector of Mode entries as "+o nick -v other" etc.
fn render_modes<T>(modes: &[irc::proto::Mode<T>]) -> String
where
    T: irc::proto::mode::ModeType,
{
    modes
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

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
        // Operator command numerics.
        Response::RPL_INVITING if args.len() >= 3 => {
            Some(format!("inviting {} to {}", p(1), p(2)))
        }
        Response::RPL_CHANNELMODEIS if args.len() >= 3 => Some(format!(
            "mode {}: {}",
            p(1),
            args[2..].join(" ")
        )),
        Response::RPL_BANLIST if args.len() >= 3 => Some(format!(
            "banlist {}: {}",
            p(1),
            args[2..].join(" ")
        )),
        Response::RPL_ENDOFBANLIST if args.len() >= 2 => {
            Some(format!("banlist {}: end", p(1)))
        }
        Response::ERR_USERNOTINCHANNEL if args.len() >= 4 => {
            Some(format!("{} is not on {}", p(1), p(2)))
        }
        Response::ERR_NOTONCHANNEL if args.len() >= 3 => {
            Some(format!("you are not on {}", p(1)))
        }
        Response::ERR_USERONCHANNEL if args.len() >= 4 => {
            Some(format!("{} is already on {}", p(1), p(2)))
        }
        Response::ERR_NEEDMOREPARAMS if args.len() >= 3 => {
            Some(format!("{} needs more parameters", p(1)))
        }
        Response::ERR_UNKNOWNMODE if args.len() >= 3 => {
            Some(format!("unknown mode char: {}", p(1)))
        }
        Response::ERR_INVITEONLYCHAN if args.len() >= 3 => {
            Some(format!("cannot join {}: invite-only", p(1)))
        }
        Response::ERR_NOPRIVILEGES => {
            Some("permission denied: server operator required".into())
        }
        // SASL flow surfacing — these come *after* CAP negotiation so the
        // status buffer exists by then.
        Response::RPL_LOGGEDIN if args.len() >= 4 => Some(format!(
            "logged in as {} ({})",
            p(2),
            args.last().map(String::as_str).unwrap_or("")
        )),
        Response::RPL_LOGGEDOUT => {
            Some("logged out of services".into())
        }
        Response::RPL_SASLSUCCESS => Some("SASL authentication successful".into()),
        Response::ERR_SASLFAIL => Some(format!(
            "SASL authentication failed: {}",
            args.last().map(String::as_str).unwrap_or("")
        )),
        _ => None,
    }
}

// TAGMSG dispatch — the protocol uses tag-only PRIVMSG-like messages.
// We look for the tags we know about and emit the appropriate event.
fn parse_tagmsg_event(
    nick: &str,
    target: &str,
    tags: &[(String, Option<String>)],
) -> Option<Vec<Event>> {
    let mut typing: Option<TypingState> = None;
    let mut reply_msgid: Option<String> = None;
    let mut react_emoji: Option<String> = None;
    for (k, v) in tags {
        match (k.as_str(), v.as_deref()) {
            ("+typing", Some("active")) => typing = Some(TypingState::Active),
            ("+typing", Some("paused")) => typing = Some(TypingState::Paused),
            ("+typing", Some("done")) => typing = Some(TypingState::Done),
            ("+draft/reply", Some(id)) if !id.is_empty() => {
                reply_msgid = Some(id.to_string())
            }
            ("+draft/react", Some(e)) if !e.is_empty() => react_emoji = Some(e.to_string()),
            _ => {}
        }
    }
    if let (Some(msgid), Some(emoji)) = (reply_msgid, react_emoji) {
        return Some(vec![Event::Reaction {
            target: target.to_string(),
            target_msgid: msgid,
            nick: nick.to_string(),
            emoji,
        }]);
    }
    if let Some(state) = typing {
        return Some(vec![Event::TypingChanged {
            target: target.to_string(),
            nick: nick.to_string(),
            state,
        }]);
    }
    None
}

// Parse a single ISUPPORT token (`KEY` or `KEY=value`) into the snapshot.
// Returns true when one of the tracked tokens actually changed.
// Parse the trailing target list from RPL_MONONLINE/731. The IRCv3
// spec allows `nick[!user@host]` per entry, comma-joined. We keep just
// the nick — the rest is informational.
fn parse_monitor_targets(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|t| t.split('!').next().unwrap_or(t).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn apply_isupport_token(isupport: &mut ISupport, tok: &str) -> bool {
    let (key, value) = match tok.split_once('=') {
        Some((k, v)) => (k, Some(v)),
        None => (tok, None),
    };
    match key {
        "MODES" => {
            let new = value.and_then(|v| v.parse::<u8>().ok());
            if isupport.modes != new {
                isupport.modes = new;
                return true;
            }
        }
        "CHANTYPES" => {
            let new = value.unwrap_or("").to_string();
            if isupport.chantypes != new {
                isupport.chantypes = new;
                return true;
            }
        }
        "PREFIX" => {
            let new = value.unwrap_or("").to_string();
            if isupport.prefix != new {
                isupport.prefix = new;
                return true;
            }
        }
        "CASEMAPPING" => {
            let new = value.unwrap_or("").to_string();
            if isupport.casemapping != new {
                isupport.casemapping = new;
                return true;
            }
        }
        "NETWORK" => {
            let new = value.map(str::to_string);
            if isupport.network != new {
                isupport.network = new;
                return true;
            }
        }
        "soju.im/FILEHOST" | "draft/FILEHOST" => {
            let new = value.map(str::to_string);
            if isupport.filehost != new {
                isupport.filehost = new;
                return true;
            }
        }
        "MONITOR" => {
            let new = value
                .and_then(|v| v.parse::<u32>().ok())
                .or(Some(u32::MAX));
            if isupport.monitor_limit != new {
                isupport.monitor_limit = new;
                return true;
            }
        }
        "CLIENTTAGDENY" => {
            let mut deny_all = false;
            let mut deny: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut allow: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for entry in value.unwrap_or("").split(',') {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                if entry == "*" {
                    deny_all = true;
                } else if let Some(rest) = entry.strip_prefix('-') {
                    allow.insert(rest.trim_start_matches('+').to_string());
                } else {
                    deny.insert(entry.trim_start_matches('+').to_string());
                }
            }
            if isupport.client_tag_deny_all != deny_all
                || isupport.client_tag_deny != deny
                || isupport.client_tag_allow != allow
            {
                isupport.client_tag_deny_all = deny_all;
                isupport.client_tag_deny = deny;
                isupport.client_tag_allow = allow;
                return true;
            }
        }
        _ => {}
    }
    false
}

// IRCv3 standard replies: FAIL/WARN/NOTE.
// Layout: `<TYPE> <command> <code> [<context>...] :<description>`.
// Returns a one-line summary or None if this isn't a standard reply.
fn format_standard_reply(cmd: &str, args: &[String]) -> Option<String> {
    let kind = match cmd.to_ascii_uppercase().as_str() {
        "FAIL" => "fail",
        "WARN" => "warn",
        "NOTE" => "note",
        _ => return None,
    };
    if args.len() < 3 {
        return None;
    }
    let command = &args[0];
    let code = &args[1];
    let description = args.last().map(String::as_str).unwrap_or("");
    let ctx = &args[2..args.len().saturating_sub(1)];
    let mut head = format!("{kind} {command} {code}");
    if !ctx.is_empty() {
        head.push_str(" [");
        head.push_str(&ctx.join(" "));
        head.push(']');
    }
    Some(format!("{head}: {description}"))
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

// Suppress connect-time noise: welcome banner, LUSERS stats, MOTD.
// Everything else not otherwise handled falls through to a raw render
// so that /raw <CMD> actually surfaces the server reply.
fn is_suppressed_numeric(code: u16) -> bool {
    matches!(code,
        1 | 2 | 3 | 4 | 5
        | 251 | 252 | 253 | 254 | 255 | 265 | 266
        | 372 | 375 | 376 | 422
    )
}

fn render_raw_numeric(code: u16, args: &[String]) -> Option<String> {
    if is_suppressed_numeric(code) {
        return None;
    }
    let body = if args.len() > 1 {
        args[1..].join(" ")
    } else {
        args.join(" ")
    };
    Some(format!("[{code}] {body}"))
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

/// Channel mode prefixes that may appear in front of nicks in NAMES,
/// in decreasing-priority order: founder, admin, op, halfop, voice.
const NAME_PREFIX_CHARS: &[char] = &['~', '&', '@', '%', '+'];

fn parse_name_entry(token: &str) -> Option<MemberEntry> {
    if token.is_empty() {
        return None;
    }
    let prefix_len = token
        .chars()
        .take_while(|c| NAME_PREFIX_CHARS.contains(c))
        .map(char::len_utf8)
        .sum();
    let (prefixes, rest) = token.split_at(prefix_len);
    if rest.is_empty() {
        return None;
    }
    // With `userhost-in-names`, rest is `nick!ident@host`. Without it, just `nick`.
    let (nick, userhost) = match rest.split_once('!') {
        Some((n, uh)) if !uh.is_empty() => (n.to_string(), Some(uh.to_string())),
        _ => (rest.to_string(), None),
    };
    Some(MemberEntry {
        nick,
        prefixes: prefixes.to_string(),
        userhost,
    })
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
