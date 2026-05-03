use futures::channel::mpsc;
use futures::{SinkExt, Stream, StreamExt};
use irc::client::prelude::*;
use irc::proto::CapSubCommand;

use crate::config::{AppConfig, AuthMode};

#[derive(Clone)]
pub enum Outgoing {
    Privmsg { target: String, text: String },
    Action { target: String, text: String },
    Ctcp { target: String, query: String },
    Join(String),
    Part { channel: String, reason: Option<String> },
    Nick(String),
}

#[derive(Clone)]
pub enum Event {
    Ready(mpsc::Sender<Outgoing>),
    Connected,
    ConnectError(String),
    Disconnected,
    Privmsg { target: String, nick: String, body: String },
    Action { target: String, nick: String, body: String },
    UserJoined { channel: String, nick: String },
    UserLeft { channel: String, nick: String },
    NickChanged { old: String, new: String },
    Names { channel: String, nicks: Vec<String> },
    Topic { channel: String, topic: String },
    Notice { from: String, text: String },
    CtcpReply { from: String, query: String, args: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthPhase {
    AwaitingCapAck,
    AwaitingChallenge,
    AwaitingResult,
    Done,
}

pub fn subscribe(cfg: &AppConfig) -> impl Stream<Item = Event> + Send + 'static {
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

        let mut auth_phase;

        if use_sasl {
            // Pause registration and request SASL. Server replies with
            // CAP * ACK :sasl, then we drive AUTHENTICATE inside the loop.
            if let Err(e) = sender.send(Command::CAP(None, CapSubCommand::REQ, None, Some("sasl".to_string()))) {
                let _ = out.send(Event::ConnectError(e.to_string())).await;
                return;
            }
            auth_phase = AuthPhase::AwaitingCapAck;
            let mech = match auth_mode {
                AuthMode::SaslExternal => "EXTERNAL",
                _ => "PLAIN",
            };
            let _ = out
                .send(Event::Notice {
                    from: "*".into(),
                    text: format!("authenticating with SASL {mech}…"),
                })
                .await;
        } else {
            if let Err(e) = client.identify() {
                let _ = out.send(Event::ConnectError(e.to_string())).await;
                return;
            }
            let _ = out.send(Event::Connected).await;
            auth_phase = AuthPhase::Done;
        }

        loop {
            tokio::select! {
                incoming = stream.next() => match incoming {
                    Some(Ok(msg)) => {
                        if auth_phase != AuthPhase::Done {
                            match handle_auth_msg(&msg, &sender, &mut auth_phase, auth_mode, &cfg) {
                                AuthOutcome::Pending => {}
                                AuthOutcome::Done => {
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
                        for ev in translate(msg) {
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
                        None => {}
                    }
                }
            }
        }
    })
}

enum AuthOutcome {
    Pending,
    Done,
    Failed(String),
}

fn handle_auth_msg(
    msg: &Message,
    sender: &Sender,
    phase: &mut AuthPhase,
    mode: AuthMode,
    cfg: &AppConfig,
) -> AuthOutcome {
    match &msg.command {
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
            if !listed.split_whitespace().any(|c| c.eq_ignore_ascii_case("sasl")) {
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
        }
        Command::CAP(_, sub, _, _) if *sub == CapSubCommand::NAK => {
            AuthOutcome::Failed("server refused SASL capability".into())
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

fn translate(msg: Message) -> Vec<Event> {
    let nick = match &msg.prefix {
        Some(Prefix::Nickname(n, _, _)) => n.clone(),
        Some(Prefix::ServerName(s)) => s.clone(),
        None => "*".into(),
    };
    match msg.command {
        Command::PRIVMSG(target, body) => {
            if let Some(action) = unwrap_ctcp_action(&body) {
                vec![Event::Action { target, nick, body: strip_irc_formatting(&action) }]
            } else if is_ctcp_wrapped(&body) {
                // Other CTCP queries (VERSION, PING, TIME, etc.) — auto-handled
                // by the irc crate, suppress the raw echo from the chat view.
                vec![]
            } else {
                vec![Event::Privmsg { target, nick, body: strip_irc_formatting(&body) }]
            }
        }
        Command::JOIN(channel, _, _) => vec![Event::UserJoined { channel, nick }],
        Command::PART(channel, _) => vec![Event::UserLeft { channel, nick }],
        Command::NICK(new) => vec![Event::NickChanged { old: nick, new }],
        Command::TOPIC(channel, Some(topic)) => {
            vec![Event::Topic { channel, topic: strip_irc_formatting(&topic) }]
        }
        Command::NOTICE(_, text) => match unwrap_ctcp_reply(&text) {
            Some((query, args)) => vec![Event::CtcpReply { from: nick, query, args }],
            None => vec![Event::Notice { from: nick, text: strip_irc_formatting(&text) }],
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
            _ => vec![],
        },
        _ => vec![],
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
