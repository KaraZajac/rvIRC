//! Parse :command lines and dispatch to IRC or internal actions.

use crate::app::UserAction;

#[derive(Debug)]
#[allow(dead_code)]
pub enum CommandResult {
    SendPrivmsg { target: String, text: String },
    Join(String),
    Part(Option<String>),
    List,
    Servers,
    Connect(String),
    Quit(()),
    Msg { nick: String, text: String },
    SwitchChannel(String),
    UserAction { nick: String, action: UserAction },
    StatusMessage(String),
    ChannelPanelShow,
    ChannelPanelHide,
    UserPanelShow,
    UserPanelHide,
    FocusChannels,
    FocusUsers,
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
            let channel = rest.split_whitespace().next().unwrap_or("").to_string();
            if channel.starts_with('#') || channel.starts_with('&') {
                CommandResult::Join(channel)
            } else if !channel.is_empty() {
                CommandResult::Join(format!("#{}", channel))
            } else {
                CommandResult::StatusMessage("Usage: :join #channel".to_string())
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
        "quit" | "exit" => CommandResult::Quit(()),
        "q" if rest.trim().is_empty() => CommandResult::Quit(()),
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
        _ => CommandResult::Unknown(format!("Unknown command: {}", cmd)),
    }
}
