//! Parse :command lines and dispatch to IRC or internal actions.

use crate::app::UserAction;

#[derive(Debug)]
#[allow(dead_code)]
pub enum CommandResult {
    SendPrivmsg { target: String, text: String },
    Join { channel: String, key: Option<String> },
    Part(Option<String>),
    List,
    Servers,
    Connect(String),
    Reconnect,
    Quit(()),
    Msg { nick: String, text: String },
    Me(String), // ACTION to current target
    Nick(String),
    Topic(Option<String>), // None = request, Some = set
    Kick { channel: Option<String>, nick: String, reason: Option<String> },
    Ban { channel: Option<String>, mask: String },
    SwitchChannel(String),
    UserAction { nick: String, action: UserAction },
    StatusMessage(String),
    ChannelPanelShow,
    ChannelPanelHide,
    UserPanelShow,
    UserPanelHide,
    FocusChannels,
    FocusUsers,
    Version,
    Credits,
    License,
    NoOp,
    Unknown(String),
}

/// Parse input after the leading ':' (e.g. "join #chan" -> Join("#chan")).
pub fn parse(line: &str) -> CommandResult {
    let line = line.trim();
    if line.is_empty() {
        return CommandResult::NoOp;
    }
    let mut words = line.splitn(2, char::is_whitespace);
    let cmd = words.next().unwrap_or("").to_lowercase();
    let rest = words.next().unwrap_or("").trim();

    match cmd.as_str() {
        "join" => {
            let mut parts = rest.split_whitespace();
            let channel = parts.next().unwrap_or("").to_string();
            let key = parts.next().map(String::from);
            if channel.starts_with('#') || channel.starts_with('&') {
                CommandResult::Join { channel, key }
            } else if !channel.is_empty() {
                CommandResult::Join { channel: format!("#{}", channel), key }
            } else {
                CommandResult::StatusMessage("Usage: :join #channel [key]".to_string())
            }
        }
        "part" | "leave" => {
            let ch = rest.split_whitespace().next().map(String::from);
            CommandResult::Part(ch)
        }
        "list" => CommandResult::List,
        "servers" => CommandResult::Servers,
        "connect" | "server" => {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if name.is_empty() {
                CommandResult::StatusMessage("Usage: :connect <server name>".to_string())
            } else {
                CommandResult::Connect(name)
            }
        }
        "reconnect" => CommandResult::Reconnect,
        "quit" | "exit" => CommandResult::Quit(()),
        "q" if rest.trim().is_empty() => CommandResult::Quit(()),
        "me" => {
            if rest.is_empty() {
                CommandResult::StatusMessage("Usage: :me <action text>".to_string())
            } else {
                CommandResult::Me(rest.to_string())
            }
        }
        "nick" => {
            let new_nick = rest.split_whitespace().next().unwrap_or("").to_string();
            if new_nick.is_empty() {
                CommandResult::StatusMessage("Usage: :nick <newnick>".to_string())
            } else {
                CommandResult::Nick(new_nick)
            }
        }
        "topic" => {
            let topic = rest.trim();
            if topic.is_empty() {
                CommandResult::Topic(None)
            } else {
                CommandResult::Topic(Some(topic.to_string()))
            }
        }
        "kick" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                CommandResult::StatusMessage("Usage: :kick <nick> [reason] or :kick #channel nick [reason]".to_string())
            } else if parts.len() == 1 {
                CommandResult::Kick { channel: None, nick: parts[0].to_string(), reason: None }
            } else if parts[0].starts_with('#') || parts[0].starts_with('&') {
                let channel = Some(parts[0].to_string());
                let nick = parts.get(1).unwrap_or(&"").to_string();
                let reason = parts.get(2).map(|s| s.to_string());
                CommandResult::Kick { channel, nick, reason }
            } else {
                let nick = parts[0].to_string();
                let reason = parts.get(1).map(|s| s.to_string());
                CommandResult::Kick { channel: None, nick, reason }
            }
        }
        "ban" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                CommandResult::StatusMessage("Usage: :ban <mask> or :ban #channel <mask>".to_string())
            } else if parts[0].starts_with('#') || parts[0].starts_with('&') {
                let channel = Some(parts[0].to_string());
                let mask = parts.get(1).unwrap_or(&"").to_string();
                CommandResult::Ban { channel, mask }
            } else {
                CommandResult::Ban { channel: None, mask: parts[0].to_string() }
            }
        }
        "msg" | "message" | "query" => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let nick = parts.next().unwrap_or("").to_string();
            let text = parts.next().unwrap_or("").to_string();
            if nick.is_empty() {
                CommandResult::StatusMessage("Usage: :msg <nick> <message>".to_string())
            } else {
                CommandResult::Msg { nick, text }
            }
        }
        "channel" | "chan" | "c" => {
            let ch = rest.split_whitespace().next().unwrap_or("").to_string();
            if ch.starts_with('#') || ch.starts_with('&') {
                CommandResult::SwitchChannel(ch)
            } else if !ch.is_empty() {
                CommandResult::SwitchChannel(format!("#{}", ch))
            } else {
                CommandResult::NoOp
            }
        }
        "channel-panel" => {
            let sub = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            match sub.as_str() {
                "show" => CommandResult::ChannelPanelShow,
                "hide" => CommandResult::ChannelPanelHide,
                _ => CommandResult::StatusMessage("Usage: :channel-panel show|hide".to_string()),
            }
        }
        "user-panel" => {
            let sub = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            match sub.as_str() {
                "show" => CommandResult::UserPanelShow,
                "hide" => CommandResult::UserPanelHide,
                _ => CommandResult::StatusMessage("Usage: :user-panel show|hide".to_string()),
            }
        }
        "channels" => CommandResult::FocusChannels,
        "users" => CommandResult::FocusUsers,
        "version" => CommandResult::Version,
        "credits" => CommandResult::Credits,
        "license" => CommandResult::License,
        _ => CommandResult::Unknown(format!("Unknown command: {}", cmd)),
    }
}
