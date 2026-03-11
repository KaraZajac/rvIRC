//! IRC connection wrapper: build irc::Config from our config, run stream, push messages to app.

use crate::app::{MessageKind, MessageLine};
use crate::config::{RvConfig, ServerEntry};
use irc::client::data::Config as IrcConfig;
use irc::client::prelude::*;
use irc::client::ClientStream;
use irc::proto::{Command as IrcCommand, Response};
use std::collections::HashMap;
use tokio::sync::mpsc;

pub type IrcMessageTx = mpsc::UnboundedSender<IrcMessage>;

#[derive(Debug)]
#[allow(dead_code)]
pub enum IrcMessage {
    Line { target: String, line: MessageLine },
    JoinedChannel(String),
    PartedChannel(String),
    UserList { channel: String, users: Vec<String> },
    /// (channel name, optional user count from LIST)
    ChannelList(Vec<(String, Option<u32>)>),
    Connected { server: String },
    Disconnected,
    WhoisResult { nick: String, lines: Vec<String> },
}

fn server_entry_to_irc_config(entry: &ServerEntry, rv: &RvConfig) -> IrcConfig {
    let mut cfg = IrcConfig::default();
    cfg.server = Some(entry.host.clone());
    cfg.port = Some(entry.port);
    cfg.use_tls = Some(entry.tls);
    cfg.nickname = rv.nickname.clone().or_else(|| Some("rvirc".to_string()));
    cfg.username = rv.username.clone().or_else(|| cfg.nickname.clone());
    cfg.realname = rv.real_name.clone().or_else(|| cfg.nickname.clone());
    cfg.password = entry.password.clone();
    cfg.channels = vec![];
    cfg
}

pub fn connect(
    server_entry: &ServerEntry,
    rv_config: &RvConfig,
    tx: IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) -> Result<(Client, ClientStream), String> {
    let irc_config = server_entry_to_irc_config(server_entry, rv_config);
    let (client, stream) = rt
        .block_on(async {
            let mut client = Client::from_config(irc_config).await.map_err(|e| e.to_string())?;
            client.identify().map_err(|e| e.to_string())?;
            let stream = client.stream().map_err(|e| e.to_string())?;
            Ok::<_, String>((client, stream))
        })?;
    let _ = tx.send(IrcMessage::Connected {
        server: server_entry.name.clone(),
    });
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
    Some((
        target,
        MessageLine { source, text, kind },
    ))
}


/// Run the IRC stream in a loop and send parsed messages to `tx`.
/// Call this from a tokio task; it runs until the stream ends.
pub async fn run_stream(mut stream: ClientStream, tx: IrcMessageTx) {
    use futures_util::StreamExt;
    let mut pending_users: HashMap<String, Vec<String>> = HashMap::new();
    let mut pending_list: Vec<(String, Option<u32>)> = Vec::new();
    let mut pending_whois: Vec<String> = Vec::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => {
                use irc::proto::Command as C;
                match &msg.command {
                    C::Response(Response::RPL_NAMREPLY, args) => {
                        if args.len() >= 4 {
                            let channel = args[2].clone();
                            let nicks: Vec<String> = args[3]
                                .split_whitespace()
                                .map(|s| s.to_string())
                                .collect();
                            pending_users
                                .entry(channel)
                                .or_default()
                                .extend(nicks);
                        }
                    }
                    C::Response(Response::RPL_ENDOFNAMES, args) => {
                        if args.len() >= 2 {
                            let channel = args[1].clone();
                            if let Some(users) = pending_users.remove(&channel) {
                                let _ = tx.send(IrcMessage::UserList { channel, users });
                            }
                        }
                    }
                    C::Response(Response::ERR_NOSUCHNICK, args) => {
                        if args.len() >= 2 {
                            pending_whois.push(format!("{} :No such nick/channel", args[1]));
                        }
                    }
                    C::Response(Response::RPL_WHOISUSER, args) => {
                        if args.len() >= 5 {
                            pending_whois.clear(); // 311 starts a new whois reply; drop any prior 401 etc.
                            let real = args.get(5).cloned().unwrap_or_else(|| "*".to_string());
                            pending_whois.push(format!(
                                "{} ({}@{}) * :{}",
                                args[1], args[2], args[3], real
                            ));
                        }
                    }
                    C::Response(Response::RPL_WHOISSERVER, args) => {
                        if args.len() >= 3 {
                            let info = args.get(3).cloned().unwrap_or_default();
                            pending_whois.push(format!("{} {} :{}", args[1], args[2], info));
                        }
                    }
                    C::Response(Response::RPL_WHOISOPERATOR, args) => {
                        if args.len() >= 2 {
                            pending_whois.push(format!("{} :is an IRC operator", args[1]));
                        }
                    }
                    C::Response(Response::RPL_WHOISIDLE, args) => {
                        if args.len() >= 3 {
                            let comment = args.get(4).cloned().unwrap_or_else(|| "seconds idle".to_string());
                            pending_whois.push(format!("{} {} :{}", args[1], args[2], comment));
                        }
                    }
                    C::Response(Response::RPL_WHOISCHANNELS, args) => {
                        if args.len() >= 3 {
                            pending_whois.push(format!("{} :{}", args[1], args[2]));
                        }
                    }
                    C::Response(Response::RPL_ENDOFWHOIS, args) => {
                        if args.len() >= 2 {
                            let nick = args[1].clone();
                            let lines = std::mem::take(&mut pending_whois);
                            let _ = tx.send(IrcMessage::WhoisResult { nick, lines });
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
                        let _ = tx.send(IrcMessage::ChannelList(list));
                    }
                    _ => {}
                }

                if let Some((target, line)) = message_line(&msg) {
                    let _ = tx.send(IrcMessage::Line {
                        target: target.clone(),
                        line: line.clone(),
                    });
                    match &msg.command {
                        C::JOIN(chan, _, _) => {
                            let _ = tx.send(IrcMessage::JoinedChannel(chan.clone()));
                        }
                        C::PART(chan, _) => {
                            let _ = tx.send(IrcMessage::PartedChannel(chan.clone()));
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(IrcMessage::Disconnected);
}
