//! IRC connection wrapper: build irc::Config from our config, run stream, push messages to app.

use crate::app::{MessageKind, MessageLine};
use crate::config::{RvConfig, ServerEntry};
use irc::client::data::Config as IrcConfig;
use irc::client::prelude::*;
use irc::proto::caps::Capability;
use irc::proto::command::CapSubCommand;
use irc::client::ClientStream;
use irc::proto::{Command as IrcCommand, Response};
use base64::Engine;
use std::collections::HashMap;
use tokio::sync::mpsc;

pub type IrcMessageTx = mpsc::UnboundedSender<IrcMessage>;

#[derive(Debug)]
#[allow(dead_code)]
pub enum IrcMessage {
    /// Server acknowledged these caps (e.g. echo-message, multi-prefix).
    CapsAcked { server: String, caps: Vec<String> },
    /// Server rejected these caps (NAK).
    CapsNak { server: String, caps: Vec<String> },
    /// Caps we requested (for display: requested - acked = nakd).
    CapRequested { server: String, caps: Vec<String> },
    Line { server: String, target: String, line: MessageLine },
    JoinedChannel { server: String, channel: String },
    PartedChannel { server: String, channel: String },
    UserList { server: String, channel: String, users: Vec<String>, userhosts: Vec<(String, String)> },
    /// (channel name, optional user count from LIST)
    ChannelList { server: String, list: Vec<(String, Option<u32>)> },
    Connected { server: String },
    Disconnected { server: String },
    WhoisResult { server: String, nick: String, lines: Vec<String> },
    /// Channel topic (332). None = no topic (331).
    Topic { server: String, channel: String, topic: Option<String> },
    /// Channel modes (324): +modes mode_params
    ChannelModes { server: String, channel: String, modes: String },
    /// INVITE inviter target channel (target may be us or another user with invite-notify)
    Invite { server: String, inviter: String, target: String, channel: String },
    /// 433: nickname in use, try alt
    NickInUse { server: String },
    /// Incoming CTCP request (VERSION, PING, TIME) - main loop sends reply.
    CtcpRequest { server: String, from_nick: String, target: String, tag: String, data: String },
    /// Send a PRIVMSG from an async task (e.g. wormhole code relay).
    SendPrivmsg { server: String, target: String, text: String },
    /// Send to target, encrypting if a secure session exists (e.g. wormhole offer).
    SendPrivmsgOrEncrypt { server: String, target: String, text: String },
    /// Status message from an async task (no server - global status bar).
    Status(String),
    /// In-chat log message (displayed as a system message in a DM/channel window).
    ChatLog { server: String, target: String, text: String },
    /// Downloaded image ready for inline display.
    ImageReady { image_id: usize, image: image::DynamicImage },
    /// Animated GIF ready: pre-decoded frames + per-frame delays.
    AnimatedImageReady {
        image_id: usize,
        frames: Vec<image::DynamicImage>,
        delays: Vec<std::time::Duration>,
    },
    /// Wormhole transfer progress (bytes done, total). Throttled in callback.
    TransferProgress {
        server: String,
        nick: String,
        filename: String,
        bytes: u64,
        total: u64,
        is_send: bool,
    },
    /// Wormhole transfer finished (success or failure). Hides progress popup.
    TransferComplete { server: String, nick: String, filename: String, is_send: bool, success: bool },
    /// MONITOR 730: nicks came online.
    MonOnline { server: String, nicks: Vec<String> },
    /// MONITOR 731: nicks went offline.
    MonOffline { server: String, nicks: Vec<String> },
    /// MONITOR 734: monitor list full, targets could not be added.
    MonListFull { server: String, limit: String, targets: String },
    /// away-notify: nick set or cleared away status. away=true if away, false if back.
    FriendAway { server: String, nick: String, away: bool },
    /// account-notify: nick logged in (Some) or out (None).
    AccountUpdate { server: String, nick: String, account: Option<String> },
    /// IRCv3 typing indicator: nick typing in target with status (active|paused|done).
    Typing { server: String, nick: String, target: String, status: String },
    /// Request chat history for target (channel or DM). Emitted when we join a channel.
    RequestChathistory { server: String, target: String },
    /// CHATHISTORY BEFORE for scroll-back. reference is "msgid=xxx" or "timestamp=YYYY-MM-DDThh:mm:ss.sssZ"
    RequestChathistoryBefore { server: String, target: String, reference: String },
    /// cap-notify: server sent CAP NEW/DEL; caps added or removed.
    CapsChanged { server: String, added: Vec<String>, removed: Vec<String> },
    /// chghost: nick changed username/host (no QUIT+JOIN).
    ChgHost { server: String, nick: String, new_user: String, new_host: String },
    /// setname: nick changed realname.
    Setname { server: String, nick: String, realname: String },
    /// Chat history batch from server (draft/chathistory). Prepend to target buffer.
    ChathistoryBatch { server: String, target: String, lines: Vec<MessageLine> },
    /// STS policy received on secure connection (duration=). Main loop persists to file.
    StsPolicyReceived { host: String, port: u16, duration_secs: u64 },
    /// STS upgrade required: connected over plain, server sent sts=port=. Reconnect with TLS.
    StsUpgradeRequired { server: String, host: String, port: u16 },
    /// Standard-replies: FAIL/WARN/NOTE from server (structured errors, warnings, notes).
    StandardReply {
        server: String,
        kind: StandardReplyKind,
        command: String,
        code: String,
        description: String,
    },
    /// REDACT: message was redacted, remove or replace in buffer.
    MessageRedacted { server: String, target: String, msgid: String },
    /// draft/message-edit: a message was edited (new_text is the replacement).
    MessageEdited { server: String, target: String, msgid: String, new_text: String },
    /// draft/message-delete: a message was deleted.
    MessageDeleted { server: String, target: String, msgid: String },
    /// draft/react: nick reacted to msgid with emoji (or unreacted).
    Reaction {
        server: String,
        target: String,
        msgid: String,
        nick: String,
        emoji: String,
        unreact: bool,
    },
    /// WHOX (354): extended WHO result — nick, user, host, optional account.
    WhoxResult { server: String, nick: String, user: String, host: String, account: Option<String> },
    /// RPL_ISUPPORT (005): server advertises UTF8ONLY token.
    IsupportUtf8Only { server: String },
    /// draft/read-marker: MARKREAD sync from server (another client moved read position).
    ReadMarker { server: String, target: String, timestamp: String },
    /// draft/channel-rename: channel was renamed in-place (no part/rejoin needed).
    ChannelRenamed { server: String, old: String, new: String, reason: Option<String> },
    /// RPL_BANLIST (367) + RPL_ENDOFBANLIST (368): full ban list for a channel.
    BanList { server: String, channel: String, entries: Vec<String> },
    /// draft/account-registration: result of REGISTER command (920/927/928 numerics).
    RegisterResult { server: String, success: bool, message: String },
    /// draft/extended-isupport / draft/network-icon: NETWORK= and NETWORKICON= from RPL_ISUPPORT (005).
    IsupportNetworkInfo { server: String, network_name: Option<String>, network_icon: Option<String> },
    /// draft/metadata: server pushed a metadata key-value for a target (value=None means cleared).
    MetadataValue { server: String, target: String, key: String, value: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardReplyKind {
    Fail,
    Warn,
    Note,
}

fn cap_to_name(c: &Capability) -> String {
    c.as_ref().to_lowercase()
}

/// Extract value from an ISUPPORT token like `KEY=value`, case-insensitive key match.
fn isupport_val<'a>(token: &'a str, key: &str) -> Option<&'a str> {
    let prefix_len = key.len() + 1; // "KEY="
    if token.len() > prefix_len && token[..prefix_len].eq_ignore_ascii_case(&format!("{}=", key)) {
        Some(&token[prefix_len..])
    } else {
        None
    }
}

fn server_entry_to_irc_config(entry: &ServerEntry, rv: &RvConfig, sts_policies: &crate::sts::StsPolicies) -> IrcConfig {
    use irc::client::data::ProxyType;

    let (port, use_tls) = if let Some((policy_port, _)) = sts_policies.get_valid(&entry.host) {
        (policy_port, true)
    } else {
        (entry.port, entry.tls)
    };

    let mut cfg = IrcConfig::default();
    cfg.server = Some(entry.host.clone());
    cfg.port = Some(port);
    cfg.use_tls = Some(use_tls);
    cfg.nickname = rv.nickname.clone().or_else(|| Some("rvirc".to_string()));
    cfg.username = rv.username.clone().or_else(|| cfg.nickname.clone());
    cfg.realname = rv.real_name.clone().or_else(|| cfg.nickname.clone());
    cfg.password = entry.password.clone();
    cfg.channels = vec![];

    if let Some(ref url_str) = entry.proxy_url {
        if let Ok(url) = url::Url::parse(url_str) {
            let scheme = url.scheme().to_lowercase();
            if scheme == "socks5" || scheme == "socks5h" {
                cfg.proxy_type = Some(ProxyType::Socks5);
                if let Some(host) = url.host_str() {
                    cfg.proxy_server = Some(host.to_string());
                }
                if let Some(port) = url.port() {
                    cfg.proxy_port = Some(port);
                }
                if !url.username().is_empty() {
                    cfg.proxy_username = Some(url.username().to_string());
                }
                if let Some(pass) = url.password() {
                    cfg.proxy_password = Some(pass.to_string());
                }
            }
        }
    }

    cfg
}

pub fn connect(
    server_entry: &ServerEntry,
    rv_config: &RvConfig,
    tx: IrcMessageTx,
    rt: &tokio::runtime::Runtime,
    initial_away: Option<String>,
    sts_policies: &crate::sts::StsPolicies,
) -> Result<(Client, ClientStream), String> {
    use futures_util::StreamExt;

    let irc_config = server_entry_to_irc_config(server_entry, rv_config, sts_policies);
    let sasl_mechanism = server_entry
        .sasl_mechanism
        .as_deref()
        .map(|s| s.to_lowercase());

    let (client, stream, acked_caps, requested) = rt.block_on(async {
        let mut client = Client::from_config(irc_config.clone()).await.map_err(|e| e.to_string())?;

        // Some servers (e.g. Libera) require CAP LS 302 before they accept CAP REQ.
        use irc::proto::caps::NegotiationVersion;
        let _ = client.send_cap_ls(NegotiationVersion::V302);

        // Request caps one at a time; some servers (e.g. Libera) NAK batches but ACK individual caps.
        let mut caps: Vec<Capability> = vec![
            Capability::MultiPrefix,
            Capability::AccountNotify,
            Capability::AccountTag,
            Capability::ExtendedJoin,
            Capability::InviteNotify,
            Capability::CapNotify,
            Capability::ChgHost,
            Capability::AwayNotify,
            Capability::Custom("message-tags"),
            Capability::EchoMessage,
            Capability::ServerTime,
            Capability::UserhostInNames,
            Capability::Batch,
            Capability::Custom("draft/chathistory"),
            Capability::Custom("draft/message-redaction"),
            Capability::Custom("labeled-response"),
            Capability::Custom("draft/no-implicit-names"),
            Capability::Custom("draft/pre-away"),
            Capability::Custom("standard-replies"),
            Capability::Custom("setname"),
            Capability::Custom("extended-monitor"),
            Capability::Custom("draft/multiline"),
            Capability::Custom("draft/read-marker"),
            Capability::Custom("draft/account-registration"),
            Capability::Custom("draft/extended-isupport"),
            Capability::Custom("draft/metadata"),
            Capability::Custom("draft/message-edit"),
            Capability::Custom("draft/message-delete"),
            Capability::Custom("draft/channel-rename"),
        ];
        if sasl_mechanism.is_some() {
            caps.push(Capability::Sasl);
        }
        let requested: Vec<String> = caps
            .iter()
            .map(|c| cap_to_name(c))
            .collect();
        // Space CAP REQs to avoid Excess Flood (Libera and others rate-limit).
        for (i, cap) in caps.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            let _ = client.send_cap_req(std::slice::from_ref(cap));
        }

        let mut stream = client.stream().map_err(|e| e.to_string())?;
        let mut acked_caps: Vec<String> = Vec::new();

        if let Some(ref mechanism) = sasl_mechanism {
            // Custom SASL registration: consume stream until SASL complete, then send CAP END + identify.
            let nick = irc_config
                .nickname
                .as_deref()
                .unwrap_or("rvirc")
                .to_string();
            let user = irc_config
                .username
                .as_deref()
                .unwrap_or(&nick)
                .to_string();
            let real = irc_config
                .realname
                .as_deref()
                .unwrap_or(&nick)
                .to_string();
            let server_pass = irc_config.password.clone().unwrap_or_default();
            let sasl_pass = server_entry.identify_password.clone().unwrap_or_default();

            let mut sasl_acked = false;
            let mut sasl_sent_mechanism = false;

            while let Some(result) = stream.next().await {
                let msg = result.map_err(|e| e.to_string())?;
                match &msg.command {
                    IrcCommand::CAP(_, CapSubCommand::ACK, multi, params) => {
                        let acked = params.as_deref().or_else(|| multi.as_deref()).unwrap_or("");
                        for c in acked.split_whitespace() {
                            acked_caps.push(c.to_lowercase());
                        }
                        if acked.split_whitespace().any(|c| c.eq_ignore_ascii_case("sasl")) {
                            sasl_acked = true;
                            if !sasl_sent_mechanism {
                                sasl_sent_mechanism = true;
                                match mechanism.as_str() {
                                    "plain" => {
                                        let _ = client.send_sasl_plain();
                                    }
                                    "external" => {
                                        let _ = client.send_sasl_external();
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    IrcCommand::AUTHENTICATE(data) => {
                        if sasl_acked && data.trim() == "+" {
                            match mechanism.as_str() {
                                "plain" => {
                                    // RFC 4616 / IRCv3: message = [authzid] NUL authcid NUL passwd
                                    // Use authzid=authcid=nick per IRCv3 spec example (jilles\0jilles\0sesame)
                                    let creds = format!("{}\0{}\0{}", nick, nick, sasl_pass);
                                    let b64 = base64::engine::general_purpose::STANDARD
                                        .encode(creds.as_bytes());
                                    let _ = client.send(IrcCommand::AUTHENTICATE(b64));
                                }
                                "external" => {
                                    let _ = client.send(IrcCommand::AUTHENTICATE("+".to_string()));
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                if let IrcCommand::Response(code, _args) = &msg.command {
                    match code {
                        Response::RPL_SASLSUCCESS | Response::ERR_SASLFAIL => {
                            let pre_away_acked = acked_caps.iter().any(|c| c == "draft/pre-away");
                            if pre_away_acked {
                                if let Some(ref msg) = initial_away {
                                    let _ = client.send(IrcCommand::AWAY(Some(msg.clone())));
                                }
                            }
                            let _ = client.send(IrcCommand::CAP(
                                None,
                                CapSubCommand::END,
                                None,
                                None,
                            ));
                            if !server_pass.is_empty() {
                                let _ = client.send(IrcCommand::PASS(server_pass));
                            }
                            let _ = client.send(IrcCommand::NICK(nick.clone()));
                            let _ = client.send(IrcCommand::USER(
                                user.clone(),
                                "0".to_string(),
                                real.clone(),
                            ));
                            break;
                        }
                        _ => {}
                    }
                }
            }
        } else {
            client.identify().map_err(|e| e.to_string())?;
        }

        Ok::<_, String>((client, stream, acked_caps, requested))
    })?;

    let server = server_entry.name.clone();
    let _ = tx.send(IrcMessage::Connected { server: server.clone() });
    let _ = tx.send(IrcMessage::CapRequested { server: server.clone(), caps: requested.clone() });
    // Forward acked caps from SASL loop (non-SASL path gets them via run_stream)
    if !acked_caps.is_empty() {
        let _ = tx.send(IrcMessage::CapsAcked { server, caps: acked_caps });
    }
    Ok((client, stream))
}

fn prefix_nick(prefix: Option<&irc::proto::Prefix>) -> String {
    match prefix {
        Some(irc::proto::Prefix::ServerName(s)) => s.clone(),
        Some(irc::proto::Prefix::Nickname(nick, _, _)) => nick.clone(),
        None => "*".to_string(),
    }
}

fn format_message_target(msg: &irc::proto::Message) -> Option<String> {
    match &msg.command {
        IrcCommand::PRIVMSG(ref target, _) | IrcCommand::NOTICE(ref target, _) => Some(target.clone()),
        IrcCommand::JOIN(ref chan, _, _) => Some(chan.clone()),
        IrcCommand::PART(ref chan, _) => Some(chan.clone()),
        _ => msg.response_target().map(String::from),
    }
}

/// Build MessageLine from a PRIVMSG or NOTICE (for chathistory batch). Handles CTCP ACTION.
fn batch_message_to_line(msg: &irc::proto::Message) -> Option<(String, MessageLine)> {
    let source = prefix_nick(msg.prefix.as_ref());
    let timestamp = server_time_from_tags(msg.tags.as_ref());
    let (target, text, kind) = match &msg.command {
        IrcCommand::PRIVMSG(t, m) => {
            if let Some((tag, data)) = parse_ctcp(m) {
                if tag == "ACTION" {
                    (t.clone(), data, MessageKind::Action)
                } else {
                    (t.clone(), m.clone(), MessageKind::Privmsg)
                }
            } else {
                (t.clone(), m.clone(), MessageKind::Privmsg)
            }
        }
        IrcCommand::NOTICE(t, m) => (t.clone(), m.clone(), MessageKind::Notice),
        _ => return None,
    };
    let account = account_from_tags(msg.tags.as_ref()).unwrap_or(None);
    let msgid = msgid_from_tags(msg.tags.as_ref());
    let reply_to_msgid = reply_to_msgid_from_tags(msg.tags.as_ref());
    let is_bot_sender = has_bot_tag(msg.tags.as_ref());
    Some((
        target.clone(),
        MessageLine { source, text, kind, image_id: None, timestamp, account, msgid, reply_to_msgid, is_bot_sender },
    ))
}

/// Extract account from message tags (IRCv3 account-tag). "*" means logged out.
fn account_from_tags(tags: Option<&Vec<irc::proto::message::Tag>>) -> Option<Option<String>> {
    let tags = tags?;
    let v = tags.iter().find(|t| t.0 == "account").and_then(|t| t.1.as_deref())?;
    Some(if v == "*" { None } else { Some(v.to_string()) })
}

/// Extract msgid from message tags (IRCv3 message-ids).
fn msgid_from_tags(tags: Option<&Vec<irc::proto::message::Tag>>) -> Option<String> {
    let tags = tags?;
    tags.iter().find(|t| t.0 == "msgid").and_then(|t| t.1.as_ref()).map(|s| s.clone())
}

/// Extract +draft/channel-context from message tags (channel to display a DM in).
fn channel_context_from_tags(tags: Option<&Vec<irc::proto::message::Tag>>) -> Option<String> {
    let tags = tags?;
    tags.iter()
        .find(|t| t.0 == "+draft/channel-context" || t.0.eq_ignore_ascii_case("channel-context"))
        .and_then(|t| t.1.as_ref())
        .filter(|v| v.starts_with('#') || v.starts_with('&'))
        .map(|s| s.clone())
}

/// Check for bot tag (IRCv3 bot-mode). Presence indicates sender is a bot.
fn has_bot_tag(tags: Option<&Vec<irc::proto::message::Tag>>) -> bool {
    tags.map_or(false, |tags| tags.iter().any(|t| t.0.eq_ignore_ascii_case("bot")))
}

/// Extract +reply target msgid from message tags (IRCv3 reply client tag).
fn reply_to_msgid_from_tags(tags: Option<&Vec<irc::proto::message::Tag>>) -> Option<String> {
    let tags = tags?;
    tags.iter()
        .find(|t| t.0 == "+reply" || t.0.eq_ignore_ascii_case("reply"))
        .and_then(|t| t.1.as_ref())
        .map(|s| s.clone())
}

/// Extract server-time from message tags (IRCv3 server-time). Returns None if absent or parse fails.
fn server_time_from_tags(tags: Option<&Vec<irc::proto::message::Tag>>) -> Option<chrono::DateTime<chrono::Local>> {
    let tags = tags?;
    let time_str = tags.iter().find(|t| t.0 == "time").and_then(|t| t.1.as_deref())?;
    chrono::DateTime::parse_from_rfc3339(time_str)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local))
}

fn message_line(msg: &irc::proto::Message) -> Option<(String, MessageLine)> {
    let source = prefix_nick(msg.prefix.as_ref());
    let (text, kind) = match &msg.command {
        IrcCommand::PRIVMSG(_, m) => (m.clone(), MessageKind::Privmsg),
        IrcCommand::NOTICE(_, m) => (m.clone(), MessageKind::Notice),
        IrcCommand::JOIN(chan, _, _) => (format!("joined {}", chan), MessageKind::Join),
        IrcCommand::PART(chan, m) => (
            m.as_ref()
                .map(|s| format!("left {} ({})", chan, s))
                .unwrap_or_else(|| format!("left {}", chan)),
            MessageKind::Part,
        ),
        IrcCommand::QUIT(m) => (
            m.as_ref()
                .cloned()
                .unwrap_or_else(|| "quit".to_string()),
            MessageKind::Quit,
        ),
        IrcCommand::NICK(n) => (format!("is now known as {}", n), MessageKind::Nick),
        IrcCommand::UserMODE(nick, modes) => (format!("mode {} {:?}", nick, modes), MessageKind::Mode),
        IrcCommand::ChannelMODE(chan, modes) => (format!("mode {} {:?}", chan, modes), MessageKind::Mode),
        other => {
            let raw = format!("{:?}", other);
            (raw, MessageKind::Other)
        }
    };
    let target = format_message_target(msg).unwrap_or_else(|| "*server*".to_string());
    let timestamp = server_time_from_tags(msg.tags.as_ref());
    let account = account_from_tags(msg.tags.as_ref()).unwrap_or(None);
    let msgid = msgid_from_tags(msg.tags.as_ref());
    let reply_to_msgid = reply_to_msgid_from_tags(msg.tags.as_ref());
    let is_bot_sender = has_bot_tag(msg.tags.as_ref());
    Some((
        target,
        MessageLine { source, text, kind, image_id: None, timestamp, account, msgid, reply_to_msgid, is_bot_sender },
    ))
}


/// Run the IRC stream in a loop and send parsed messages to `tx`.
/// Call this from a tokio task; it runs until the stream ends.
/// `server` identifies this connection; `host` for STS policy; `use_tls` whether connection is secure.
pub async fn run_stream(
    mut stream: ClientStream,
    tx: IrcMessageTx,
    server: String,
    host: String,
    use_tls: bool,
    our_nick: Option<String>,
) {
    // our_nick is mutable so we can track the current nick across renames.
    #[allow(unused_mut)]
    let mut our_nick = our_nick;
    use irc::proto::command::BatchSubCommand;
    use futures_util::StreamExt;
    let mut pending_users: HashMap<String, (Vec<String>, Vec<(String, String)>)> = HashMap::new();
    let mut chathistory_batches: HashMap<String, (String, Vec<MessageLine>)> = HashMap::new();
    let mut netsplit_batches: HashMap<String, (Vec<String>, usize, bool)> = HashMap::new(); // id -> (server_names, count, is_netjoin)
    let mut pending_list: Vec<(String, Option<u32>)> = Vec::new();
    let mut pending_bans: HashMap<String, Vec<String>> = HashMap::new();
    #[derive(Default)]
    struct PendingWhois {
        nick: Option<String>,
        username: Option<String>,
        real_name: Option<String>,
        host: Option<String>,
        server: Option<String>,
        server_info: Option<String>,
        channels: Option<String>,
        extra: Vec<String>,
    }
    let mut pending_whois: PendingWhois = PendingWhois::default();

    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => {
                use irc::proto::Command as C;

                // BATCH handling for chathistory
                if let C::BATCH(ref batch_id, ref subcmd, ref params) = msg.command {
                    if batch_id.starts_with('+') {
                        let id = batch_id[1..].to_string();
                        if matches!(subcmd, Some(BatchSubCommand::CUSTOM(t)) if t.eq_ignore_ascii_case("chathistory"))
                            && params.as_ref().map_or(false, |p| !p.is_empty())
                        {
                            let target = params.as_ref().unwrap()[0].clone();
                            chathistory_batches.insert(id, (target, Vec::new()));
                        } else if matches!(subcmd, Some(BatchSubCommand::NETSPLIT | BatchSubCommand::NETJOIN)) {
                            let servers = params.as_deref().unwrap_or_default().to_vec();
                            let is_netjoin = matches!(subcmd, Some(BatchSubCommand::NETJOIN));
                            netsplit_batches.insert(id, (servers, 0, is_netjoin));
                        }
                    } else if batch_id.starts_with('-') {
                        let id = batch_id[1..].to_string();
                        if let Some((target, lines)) = chathistory_batches.remove(&id) {
                            let _ = tx.send(IrcMessage::ChathistoryBatch { server: server.clone(), target, lines });
                        } else if let Some((servers, count, is_netjoin)) = netsplit_batches.remove(&id) {
                            let servers_str = servers.join(" ");
                            let label = if is_netjoin { "Netjoin" } else { "Netsplit" };
                            let text = format!("{}: {} ({} user{})", label, servers_str, count, if count == 1 { "" } else { "s" });
                            let line = MessageLine {
                                source: "*".to_string(),
                                text,
                                kind: MessageKind::Other,
                                image_id: None,
                                timestamp: None,
                                account: None,
                                msgid: None,
                                reply_to_msgid: None,
                                is_bot_sender: false,
                            };
                            let _ = tx.send(IrcMessage::Line {
                                server: server.clone(),
                                target: "*server*".to_string(),
                                line,
                            });
                        }
                    }
                }

                // PRIVMSG/NOTICE with batch= tag: collect for chathistory, skip normal handling
                let in_chathistory_batch = msg.tags.as_ref().and_then(|tags| {
                    tags.iter().find(|t| t.0 == "batch").and_then(|t| t.1.clone())
                });
                if let Some(ref bid) = in_chathistory_batch {
                    if let Some((_target, ref mut lines)) = chathistory_batches.get_mut(bid) {
                        if let Some((_t, line)) = batch_message_to_line(&msg) {
                            lines.push(line);
                        }
                        continue;
                    }
                }
                // QUIT/JOIN with batch= tag: collect for netsplit/netjoin, skip normal handling
                if let Some(ref bid) = in_chathistory_batch {
                    if let Some((_servers, ref mut count, is_netjoin)) = netsplit_batches.get_mut(bid) {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        let matches_batch = (*is_netjoin && matches!(&msg.command, C::JOIN(..)))
                            || (!*is_netjoin && matches!(&msg.command, C::QUIT(_)));
                        if matches_batch && !nick.is_empty() && nick != "*" {
                            *count += 1;
                        }
                        continue;
                    }
                }

                if matches!(&msg.command, C::CAP(_, CapSubCommand::LS, _, _) | C::CAP(_, CapSubCommand::LIST, _, _) | C::CAP(_, CapSubCommand::NEW, _, _)) {
                    let list = match &msg.command {
                        C::CAP(_, _, multi, params) => params.as_deref().or_else(|| multi.as_deref()).unwrap_or(""),
                        _ => "",
                    };
                    if use_tls {
                        if let Some((port, duration)) = crate::sts::find_sts_in_cap_list(list) {
                            let _ = tx.send(IrcMessage::StsPolicyReceived {
                                host: host.clone(),
                                port,
                                duration_secs: duration,
                            });
                        }
                    } else if let Some(sts_port) = crate::sts::find_sts_upgrade_port(list) {
                        let _ = tx.send(IrcMessage::StsUpgradeRequired {
                            server: server.clone(),
                            host: host.clone(),
                            port: sts_port,
                        });
                    }
                }
                if let C::CAP(_, CapSubCommand::ACK, multi, params) = &msg.command {
                    let acked = params.as_deref().or_else(|| multi.as_deref()).unwrap_or("");
                    let caps: Vec<String> = acked.split_whitespace().map(|c| c.to_lowercase()).collect();
                    if !caps.is_empty() {
                        let _ = tx.send(IrcMessage::CapsAcked { server: server.clone(), caps });
                    }
                }
                if let C::CAP(_, CapSubCommand::NAK, multi, params) = &msg.command {
                    let nakd = params.as_deref().or_else(|| multi.as_deref()).unwrap_or("");
                    let caps: Vec<String> = nakd.split_whitespace().map(|c| c.to_lowercase()).collect();
                    if !caps.is_empty() {
                        let _ = tx.send(IrcMessage::CapsNak { server: server.clone(), caps });
                    }
                }
                if let C::CAP(_, CapSubCommand::NEW, multi, params) = &msg.command {
                    let cap_str = params.as_deref().or_else(|| multi.as_deref()).unwrap_or("");
                    let caps: Vec<String> = cap_str.split_whitespace().map(|c| c.to_lowercase()).collect();
                    if !caps.is_empty() {
                        let _ = tx.send(IrcMessage::CapsChanged {
                            server: server.clone(),
                            added: caps,
                            removed: vec![],
                        });
                    }
                }
                if let C::CAP(_, CapSubCommand::DEL, multi, params) = &msg.command {
                    let cap_str = params.as_deref().or_else(|| multi.as_deref()).unwrap_or("");
                    let caps: Vec<String> = cap_str.split_whitespace().map(|c| c.to_lowercase()).collect();
                    if !caps.is_empty() {
                        let _ = tx.send(IrcMessage::CapsChanged {
                            server: server.clone(),
                            added: vec![],
                            removed: caps,
                        });
                    }
                }

                // standard-replies: FAIL, WARN, NOTE — <type> <command> <code> [context...] :<description>
                if let C::Raw(ref cmd, ref args) = &msg.command {
                    if args.len() >= 3
                        && matches!(
                            cmd.as_str(),
                            "FAIL" | "WARN" | "NOTE"
                        )
                    {
                        let kind = match cmd.as_str() {
                            "FAIL" => StandardReplyKind::Fail,
                            "WARN" => StandardReplyKind::Warn,
                            _ => StandardReplyKind::Note,
                        };
                        let command = args[0].clone();
                        let code = args[1].clone();
                        let description = args.last().cloned().unwrap_or_default();
                        let _ = tx.send(IrcMessage::StandardReply {
                            server: server.clone(),
                            kind,
                            command,
                            code,
                            description,
                        });
                    } else if cmd.eq_ignore_ascii_case("REDACT") && args.len() >= 2 {
                        let target = args[0].clone();
                        let msgid = args[1].clone();
                        let _ = tx.send(IrcMessage::MessageRedacted {
                            server: server.clone(),
                            target,
                            msgid,
                        });
                    } else if cmd.eq_ignore_ascii_case("MARKREAD") && !args.is_empty() {
                        // draft/read-marker: MARKREAD <target> [timestamp=<iso>]
                        let target = args[0].clone();
                        let timestamp = args.get(1)
                            .and_then(|a| a.strip_prefix("timestamp="))
                            .unwrap_or("")
                            .to_string();
                        let _ = tx.send(IrcMessage::ReadMarker { server: server.clone(), target, timestamp });
                    } else if cmd.eq_ignore_ascii_case("RENAME") && args.len() >= 2 {
                        // draft/channel-rename: RENAME <old> <new> [reason]
                        let old = args[0].clone();
                        let new = args[1].clone();
                        let reason = args.get(2).map(|s| s.clone());
                        let _ = tx.send(IrcMessage::ChannelRenamed { server: server.clone(), old, new, reason });
                    } else if cmd.eq_ignore_ascii_case("METADATA") && args.len() >= 3 {
                        // draft/metadata: METADATA <target> <key> <visibility> [:<value>]
                        // Server-pushed event when someone's metadata changes.
                        let target = args[0].clone();
                        let key = args[1].clone();
                        // args[2] = visibility; args[3] = value (if present)
                        let value = args.get(3).map(|v| v.clone());
                        let _ = tx.send(IrcMessage::MetadataValue { server: server.clone(), target, key, value });
                    } else if cmd.eq_ignore_ascii_case("EDIT") && !args.is_empty() {
                        // draft/message-edit: @draft/target-msgid=<id>;draft/edit-text=<text> EDIT <target>
                        let target = args[0].clone();
                        if let Some(tags) = msg.tags.as_ref() {
                            let msgid = tags.iter().find(|t| t.0 == "draft/target-msgid").and_then(|t| t.1.clone());
                            let new_text = tags.iter().find(|t| t.0 == "draft/edit-text").and_then(|t| t.1.clone());
                            if let (Some(msgid), Some(new_text)) = (msgid, new_text) {
                                let _ = tx.send(IrcMessage::MessageEdited { server: server.clone(), target, msgid, new_text });
                            }
                        }
                    } else if cmd.eq_ignore_ascii_case("DELETE") && !args.is_empty() {
                        // draft/message-delete: @draft/target-msgid=<id> DELETE <target>
                        let target = args[0].clone();
                        if let Some(tags) = msg.tags.as_ref() {
                            let msgid = tags.iter().find(|t| t.0 == "draft/target-msgid").and_then(|t| t.1.clone());
                            if let Some(msgid) = msgid {
                                let _ = tx.send(IrcMessage::MessageDeleted { server: server.clone(), target, msgid });
                            }
                        }
                    }
                }

                match &msg.command {
                    C::Response(Response::RPL_NAMREPLY, args) => {
                        if args.len() >= 4 {
                            let channel = args[2].clone();
                            // multi-prefix + userhost-in-names: entries are @%+nick!user@host; parse to prefix+nick for display
                            let mut entries: Vec<(String, Option<(String, String)>)> = Vec::new();
                            for s in args[3].split_whitespace() {
                                let s = s.trim();
                                let prefix_end = s
                                    .bytes()
                                    .take_while(|b| matches!(b, b'~' | b'&' | b'@' | b'%' | b'+' | b'!' | b'.'))
                                    .count();
                                let (prefix, rest) = s.split_at(prefix_end);
                                let (display, userhost) = if let Some(idx) = rest.find('!') {
                                    let nick = &rest[..idx];
                                    let userhost = rest[idx + 1..].to_string();
                                    (format!("{}{}", prefix, nick), Some((nick.to_lowercase(), userhost)))
                                } else {
                                    (format!("{}{}", prefix, rest), None)
                                };
                                entries.push((display, userhost));
                            }
                            let users: Vec<String> = entries.iter().map(|(u, _)| u.clone()).collect();
                            let userhosts: Vec<(String, String)> = entries.iter().filter_map(|(_, uh)| uh.clone()).collect();
                            let (all_users, all_userhosts) = pending_users.entry(channel).or_default();
                            all_users.extend(users);
                            all_userhosts.extend(userhosts);
                        }
                    }
                    C::Response(Response::RPL_ENDOFNAMES, args) => {
                        if args.len() >= 2 {
                            let channel = args[1].clone();
                            if let Some((users, userhosts)) = pending_users.remove(&channel) {
                                let _ = tx.send(IrcMessage::UserList { server: server.clone(), channel, users, userhosts });
                            }
                        }
                    }
                    C::Response(Response::ERR_NOSUCHNICK, args) => {
                        if args.len() >= 2 {
                            pending_whois = PendingWhois::default();
                            pending_whois.nick = Some(args[1].clone());
                            pending_whois.extra.push("No such nick/channel".to_string());
                        }
                    }
                    C::Response(Response::RPL_WHOISUSER, args) => {
                        if args.len() >= 5 {
                            pending_whois = PendingWhois::default();
                            pending_whois.nick = Some(args[1].clone());
                            pending_whois.username = Some(args[2].clone());
                            pending_whois.host = Some(args[3].clone());
                            pending_whois.real_name = args.get(5).cloned();
                        }
                    }
                    C::Response(Response::RPL_WHOISSERVER, args) => {
                        if args.len() >= 3 {
                            pending_whois.server = Some(args[2].clone());
                            pending_whois.server_info = args.get(3).cloned().filter(|s| !s.is_empty());
                        }
                    }
                    C::Response(Response::RPL_WHOISOPERATOR, args) => {
                        if args.len() >= 2 {
                            pending_whois.extra.push("IRC operator".to_string());
                        }
                    }
                    C::Response(Response::RPL_WHOISIDLE, args) => {
                        if args.len() >= 3 {
                            let comment = args.get(4).cloned().unwrap_or_else(|| "seconds idle".to_string());
                            pending_whois.extra.push(format!("Idle: {} {}", args[2], comment));
                        }
                    }
                    C::Response(Response::RPL_WHOISCHANNELS, args) => {
                        if args.len() >= 3 {
                            pending_whois.channels = Some(args[2].clone());
                        }
                    }
                    C::Response(Response::RPL_ENDOFWHOIS, args) => {
                        if args.len() >= 2 {
                            let nick = args[1].clone();
                            let mut lines: Vec<String> = Vec::new();
                            if let Some(n) = pending_whois.nick.as_ref() {
                                lines.push(format!("Nick: {}", n));
                            }
                            if let Some(u) = pending_whois.username.as_ref() {
                                lines.push(format!("Username: {}", u));
                            }
                            if let Some(r) = pending_whois.real_name.as_ref() {
                                lines.push(format!("Real Name: {}", r));
                            }
                            if let Some(h) = pending_whois.host.as_ref() {
                                lines.push(format!("Host: {}", h));
                            }
                            if let Some(s) = pending_whois.server.as_ref() {
                                lines.push(format!("Server: {}", s));
                            }
                            if let Some(c) = pending_whois.channels.as_ref() {
                                lines.push(format!("Channels: {}", c));
                            }
                            if let Some(l) = pending_whois.server_info.as_ref() {
                                lines.push(format!("Location: {}", l));
                            }
                            for e in &pending_whois.extra {
                                lines.push(e.clone());
                            }
                            if lines.is_empty() {
                                lines.push("(no whois data)".to_string());
                            }
                            let _ = tx.send(IrcMessage::WhoisResult { server: server.clone(), nick, lines });
                            pending_whois = PendingWhois::default();
                        }
                    }
                    C::Response(Response::RPL_LIST, args) => {
                        if args.len() >= 2 {
                            let name = args[1].clone();
                            let count = args.get(2).and_then(|s| s.parse::<u32>().ok());
                            pending_list.push((name, count));
                        }
                    }
                    C::Response(Response::RPL_LISTEND, _) => {
                        let list = std::mem::take(&mut pending_list);
                        let _ = tx.send(IrcMessage::ChannelList { server: server.clone(), list });
                    }
                    // RPL_BANLIST (367): one entry in the ban list for a channel.
                    C::Response(Response::RPL_BANLIST, args) => {
                        if args.len() >= 3 {
                            let channel = args[1].clone();
                            let mask = args[2].clone();
                            pending_bans.entry(channel).or_default().push(mask);
                        }
                    }
                    // RPL_ENDOFBANLIST (368): end of ban list — send accumulated entries.
                    C::Response(Response::RPL_ENDOFBANLIST, args) => {
                        if args.len() >= 2 {
                            let channel = args[1].clone();
                            let entries = pending_bans.remove(&channel).unwrap_or_default();
                            let _ = tx.send(IrcMessage::BanList { server: server.clone(), channel, entries });
                        }
                    }
                    // RPL_WHOSPCRPL (354): WHOX extended WHO response.
                    // Response is #[repr(u16)] so we cast to match the non-standard numeric.
                    // Request format: WHO #chan %tuhnaf,token
                    // Response args: [our_nick, token, user, host, nick, flags, account]
                    C::Response(resp, args) if *resp as u16 == 354 => {
                        if args.len() >= 6 {
                            let nick = args[4].clone();
                            let user = args[2].clone();
                            let host = args[3].clone();
                            let account_raw = args.get(6).cloned().unwrap_or_default();
                            let account = if account_raw.is_empty() || account_raw == "0" || account_raw == "*" {
                                None
                            } else {
                                Some(account_raw)
                            };
                            if !nick.is_empty() {
                                let _ = tx.send(IrcMessage::WhoxResult { server: server.clone(), nick, user, host, account });
                            }
                        }
                    }
                    C::Response(Response::RPL_TOPIC, args) => {
                        if args.len() >= 3 {
                            let channel = args[1].clone();
                            let topic = args.get(2).cloned();
                            let _ = tx.send(IrcMessage::Topic { server: server.clone(), channel, topic });
                        }
                    }
                    C::Response(Response::RPL_NOTOPIC, args) => {
                        if args.len() >= 2 {
                            let _ = tx.send(IrcMessage::Topic {
                                server: server.clone(),
                                channel: args[1].clone(),
                                topic: None,
                            });
                        }
                    }
                    C::Response(Response::RPL_CHANNELMODEIS, args) => {
                        if args.len() >= 3 {
                            let channel = args[1].clone();
                            let modes = args[2].clone();
                            let params = args.get(3).cloned().unwrap_or_default();
                            let full = if params.is_empty() { modes } else { format!("{} {}", modes, params) };
                            let _ = tx.send(IrcMessage::ChannelModes { server: server.clone(), channel, modes: full });
                        }
                    }
                    C::Response(Response::ERR_NICKNAMEINUSE, _) => {
                        let _ = tx.send(IrcMessage::NickInUse { server: server.clone() });
                    }
                    // RPL_MYINFO is 004; 005 is RPL_BOUNCE (RFC 2812) / RPL_ISUPPORT (modern).
                    C::Response(Response::RPL_ISUPPORT, ref args) => {
                        if args.iter().any(|a| a.eq_ignore_ascii_case("UTF8ONLY")) {
                            let _ = tx.send(IrcMessage::IsupportUtf8Only { server: server.clone() });
                        }
                        // draft/extended-isupport + draft/network-icon: extract NETWORK and NETWORKICON tokens.
                        let mut network_name: Option<String> = None;
                        let mut network_icon: Option<String> = None;
                        for token in args.iter() {
                            if let Some(val) = isupport_val(token, "NETWORK") {
                                network_name = Some(val.to_string());
                            } else if let Some(val) = isupport_val(token, "NETWORKICON") {
                                network_icon = Some(val.to_string());
                            }
                        }
                        if network_name.is_some() || network_icon.is_some() {
                            let _ = tx.send(IrcMessage::IsupportNetworkInfo {
                                server: server.clone(),
                                network_name,
                                network_icon,
                            });
                        }
                    }
                    // draft/metadata: 761 RPL_KEYVALUE — [client, target, key, visibility, value].
                    C::Response(resp, ref args) if *resp as u16 == 761 => {
                        if args.len() >= 5 {
                            let target = args[1].clone();
                            let key = args[2].clone();
                            let value = Some(args[4].clone());
                            let _ = tx.send(IrcMessage::MetadataValue { server: server.clone(), target, key, value });
                        }
                    }

                    C::Response(Response::RPL_MONONLINE, args) => {
                        if args.len() >= 2 {
                            let targets = args[1].strip_prefix(':').unwrap_or(&args[1]);
                            let nicks: Vec<String> = targets
                                .split(',')
                                .map(|s| s.split('!').next().unwrap_or(s).trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if !nicks.is_empty() {
                                let _ = tx.send(IrcMessage::MonOnline { server: server.clone(), nicks });
                            }
                        }
                    }
                    C::Response(Response::RPL_MONOFFLINE, args) => {
                        if args.len() >= 2 {
                            let targets = args[1].strip_prefix(':').unwrap_or(&args[1]);
                            let nicks: Vec<String> = targets
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            if !nicks.is_empty() {
                                let _ = tx.send(IrcMessage::MonOffline { server: server.clone(), nicks });
                            }
                        }
                    }
                    C::Response(Response::ERR_MONLISTFULL, args) => {
                        if args.len() >= 3 {
                            let limit = args[1].clone();
                            let targets = args[2].strip_prefix(':').unwrap_or(&args[2]).to_string();
                            let _ = tx.send(IrcMessage::MonListFull {
                                server: server.clone(),
                                limit,
                                targets,
                            });
                        }
                    }
                    C::AWAY(ref away_msg) => {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        if !nick.is_empty() && nick != "*" {
                            let away = away_msg.is_some();
                            let _ = tx.send(IrcMessage::FriendAway { server: server.clone(), nick, away });
                        }
                    }
                    C::Raw(ref cmd, ref args) if cmd.eq_ignore_ascii_case("ACCOUNT") => {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        if !nick.is_empty() && nick != "*" {
                            let account = args.first().and_then(|a| {
                                if a == "*" { None } else { Some(a.clone()) }
                            });
                            let _ = tx.send(IrcMessage::AccountUpdate {
                                server: server.clone(),
                                nick,
                                account,
                            });
                        }
                    }
                    C::NICK(ref new_nick_raw) => {
                        // If it's our own nick being renamed, keep our_nick in sync so
                        // PART/JOIN detection and DM-redirect logic stays accurate.
                        let from_nick = prefix_nick(msg.prefix.as_ref());
                        if our_nick.as_ref().map_or(false, |n| n.eq_ignore_ascii_case(&from_nick)) {
                            our_nick = Some(new_nick_raw.clone());
                        }
                    }
                    C::Raw(ref cmd, ref args) if cmd.eq_ignore_ascii_case("CHGHOST") => {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        if !nick.is_empty() && args.len() >= 2 {
                            let new_user = args[0].clone();
                            let new_host = args[1].clone();
                            let _ = tx.send(IrcMessage::ChgHost {
                                server: server.clone(),
                                nick,
                                new_user,
                                new_host,
                            });
                        }
                    }
                    C::Raw(ref cmd, ref args) if cmd.eq_ignore_ascii_case("SETNAME") => {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        if !nick.is_empty() {
                            let realname = args.first().map(|s| s.strip_prefix(':').unwrap_or(s).to_string()).unwrap_or_default();
                            if !realname.is_empty() {
                                let _ = tx.send(IrcMessage::Setname {
                                    server: server.clone(),
                                    nick,
                                    realname,
                                });
                            }
                        }
                    }
                    C::INVITE(ref target, ref channel) => {
                        let inviter = prefix_nick(msg.prefix.as_ref());
                        let _ = tx.send(IrcMessage::Invite {
                            server: server.clone(),
                            inviter,
                            target: target.clone(),
                            channel: channel.clone(),
                        });
                    }
                    C::Raw(ref cmd, ref args) if cmd.eq_ignore_ascii_case("TAGMSG") => {
                        let nick = prefix_nick(msg.prefix.as_ref());
                        let target_opt = args.first();
                        let tags_opt = msg.tags.as_ref();
                        if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                            let _ = tx.send(IrcMessage::Status(format!(
                                "TAGMSG recv: nick={:?} target={:?} tags={:?} prefix={:?}",
                                nick,
                                target_opt,
                                tags_opt.as_ref().map(|t| t.iter().map(|x| format!("{}={:?}", x.0, x.1)).collect::<Vec<_>>().join(",")),
                                msg.prefix.as_ref().map(|p| format!("{:?}", p))
                            )));
                        }
                        if let (Some(target), Some(tags)) = (target_opt, tags_opt) {
                            let mut handled = false;
                            for tag in tags.iter() {
                                if tag.0 == "+typing" {
                                    if let Some(ref status) = tag.1 {
                                        if matches!(status.as_str(), "active" | "paused" | "done") {
                                            let _ = tx.send(IrcMessage::Typing {
                                                server: server.clone(),
                                                nick: nick.clone(),
                                                target: target.clone(),
                                                status: status.clone(),
                                            });
                                        }
                                    }
                                    handled = true;
                                    break;
                                }
                            }
                            if !handled {
                                let reply_to = tags.iter().find(|t| t.0 == "+reply" || t.0.eq_ignore_ascii_case("reply")).and_then(|t| t.1.as_ref()).cloned();
                                let react = tags.iter().find(|t| t.0.ends_with("draft/react")).and_then(|t| t.1.as_ref()).cloned();
                                let unreact = tags.iter().find(|t| t.0.ends_with("draft/unreact")).and_then(|t| t.1.as_ref()).cloned();
                                if let Some(msgid) = reply_to {
                                    if let Some(emoji) = react {
                                        let _ = tx.send(IrcMessage::Reaction {
                                            server: server.clone(),
                                            target: target.clone(),
                                            msgid: msgid.clone(),
                                            nick: nick.clone(),
                                            emoji: emoji.clone(),
                                            unreact: false,
                                        });
                                    } else if let Some(emoji) = unreact {
                                        let _ = tx.send(IrcMessage::Reaction {
                                            server: server.clone(),
                                            target: target.clone(),
                                            msgid: msgid.clone(),
                                            nick: nick.clone(),
                                            emoji: emoji.clone(),
                                            unreact: true,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    C::PRIVMSG(ref target, ref text) => {
                        if let Some((tag, data)) = parse_ctcp(text) {
                            if tag == "ACTION" {
                                let source = prefix_nick(msg.prefix.as_ref());
                                let timestamp = server_time_from_tags(msg.tags.as_ref());
                                let account = account_from_tags(msg.tags.as_ref()).unwrap_or(None);
                                let msgid = msgid_from_tags(msg.tags.as_ref());
                                let reply_to_msgid = reply_to_msgid_from_tags(msg.tags.as_ref());
                                let is_bot_sender = has_bot_tag(msg.tags.as_ref());
                                let action_target = if !target.starts_with('#') && !target.starts_with('&')
                                    && our_nick.as_ref().map_or(false, |n| target.eq_ignore_ascii_case(n))
                                {
                                    channel_context_from_tags(msg.tags.as_ref()).unwrap_or_else(|| target.clone())
                                } else {
                                    target.clone()
                                };
                                let _ = tx.send(IrcMessage::Line {
                                    server: server.clone(),
                                    target: action_target,
                                    line: MessageLine {
                                        source,
                                        text: data,
                                        kind: MessageKind::Action,
                                        image_id: None,
                                        timestamp,
                                        account,
                                        msgid,
                                        reply_to_msgid,
                                        is_bot_sender,
                                    },
                                });
                            } else if matches!(tag.as_str(), "VERSION" | "PING" | "TIME") {
                                let from_nick = prefix_nick(msg.prefix.as_ref());
                                let _ = tx.send(IrcMessage::CtcpRequest {
                                    server: server.clone(),
                                    from_nick,
                                    target: target.clone(),
                                    tag: tag.clone(),
                                    data: data.clone(),
                                });
                            }
                        }
                    }
                    _ => {}
                }

                let should_skip_line = matches!(&msg.command, C::PRIVMSG(_, t) if parse_ctcp(t).is_some())
                    || matches!(&msg.command, C::Response(_, _))
                    || matches!(&msg.command, C::AWAY(_))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("TAGMSG"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("ACCOUNT"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("CHGHOST"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("SETNAME"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("MARKREAD"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("RENAME"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("METADATA"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("EDIT"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if cmd.eq_ignore_ascii_case("DELETE"))
                    || matches!(&msg.command, C::Raw(ref cmd, _) if matches!(cmd.as_str(), "FAIL" | "WARN" | "NOTE"));
                if !should_skip_line {
                    if let Some((target, line)) = message_line(&msg) {
                        let effective_target = if !target.starts_with('#') && !target.starts_with('&')
                            && our_nick.as_ref().map_or(false, |n| target.eq_ignore_ascii_case(n))
                        {
                            channel_context_from_tags(msg.tags.as_ref()).unwrap_or(target)
                        } else {
                            target.clone()
                        };
                        let _ = tx.send(IrcMessage::Line {
                            server: server.clone(),
                            target: effective_target,
                            line: line.clone(),
                        });
                        match &msg.command {
                            C::JOIN(chan, account_opt, realname_opt) => {
                                let _ = tx.send(IrcMessage::JoinedChannel { server: server.clone(), channel: chan.clone() });
                                if our_nick.as_deref() == Some(prefix_nick(msg.prefix.as_ref()).as_str()) {
                                    let _ = tx.send(IrcMessage::RequestChathistory { server: server.clone(), target: chan.clone() });
                                }
                                // extended-join: exactly 3 params (channel, account, realname) — not channel+key
                                if let (Some(acc), Some(_)) = (account_opt, realname_opt) {
                                    let nick = prefix_nick(msg.prefix.as_ref());
                                    if !nick.is_empty() && nick != "*" {
                                        let account = if acc == "*" { None } else { Some(acc.clone()) };
                                        let _ = tx.send(IrcMessage::AccountUpdate {
                                            server: server.clone(),
                                            nick,
                                            account,
                                        });
                                    }
                                }
                            }
                            C::PART(chan, _) => {
                                // Only remove channel from our list when *we* part, not when others part
                                if our_nick.as_deref() == Some(prefix_nick(msg.prefix.as_ref()).as_str()) {
                                    let _ = tx.send(IrcMessage::PartedChannel { server: server.clone(), channel: chan.clone() });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(IrcMessage::Disconnected { server });
}

/// Parse CTCP (\x01TAG data\x01). Returns (TAG, data) or None.
fn parse_ctcp(text: &str) -> Option<(String, String)> {
    let t = text.trim();
    if !t.starts_with('\x01') || !t.ends_with('\x01') {
        return None;
    }
    let inner = t[1..t.len() - 1].trim();
    let mut split = inner.splitn(2, char::is_whitespace);
    let tag = split.next()?.to_string();
    let data = split.next().unwrap_or("").to_string();
    Some((tag, data))
}
