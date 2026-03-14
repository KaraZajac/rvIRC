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
    Disconnect,
    Quit(()),
    Msg { nick: String, text: String },
    Me(String), // ACTION to current target
    Nick(String),
    Topic(Option<String>), // None = request, Some = set
    Kick { channel: Option<String>, nick: String, reason: Option<String> },
    Ban { channel: Option<String>, mask: String },
    Unban { channel: Option<String>, mask: String },
    Invite { nick: String, channel: Option<String> },
    SwitchChannel(String),
    Away(Option<String>),
    UserAction { nick: String, action: UserAction },
    StatusMessage(String),
    ChannelPanelShow,
    ChannelPanelHide,
    MessagesPanelShow,
    MessagesPanelHide,
    UserPanelShow,
    UserPanelHide,
    FriendsPanelShow,
    FriendsPanelHide,
    FocusChannels,
    FocusMessages,
    FocusUsers,
    FocusFriends,
    AddFriend(String),
    RemoveFriend(String),
    Version,
    Credits,
    License,
    Whois(String), // nick, empty = default to current DM target
    Secure(String),
    Unsecure(String),
    Verify(String),
    Verified(String),
    SendFile { nick: String, path: String },
    Clear,
    Search,
    Highlight,
    Ignore(String),
    Unignore(String),
    NotificationsOn,
    NotificationsOff,
    Mute,
    Unmute,
    DebugTyping,
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
        "disconnect" => CommandResult::Disconnect,
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
        "unban" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                CommandResult::StatusMessage("Usage: :unban <mask> or :unban #channel <mask>".to_string())
            } else if parts[0].starts_with('#') || parts[0].starts_with('&') {
                let channel = Some(parts[0].to_string());
                let mask = parts.get(1).unwrap_or(&"").to_string();
                CommandResult::Unban { channel, mask }
            } else {
                CommandResult::Unban { channel: None, mask: parts[0].to_string() }
            }
        }
        "invite" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                CommandResult::StatusMessage("Usage: :invite <nick> [#channel]".to_string())
            } else if parts[0].starts_with('#') || parts[0].starts_with('&') {
                CommandResult::StatusMessage("Usage: :invite <nick> [#channel]".to_string())
            } else if parts.len() >= 2 {
                let nick = parts[0].to_string();
                let channel = if parts[1].starts_with('#') || parts[1].starts_with('&') {
                    Some(parts[1].to_string())
                } else {
                    Some(format!("#{}", parts[1]))
                };
                CommandResult::Invite { nick, channel }
            } else {
                CommandResult::Invite { nick: parts[0].to_string(), channel: None }
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
        "messages-panel" => {
            let sub = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            match sub.as_str() {
                "show" => CommandResult::MessagesPanelShow,
                "hide" => CommandResult::MessagesPanelHide,
                _ => CommandResult::StatusMessage("Usage: :messages-panel show|hide".to_string()),
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
        "friends-panel" => {
            let sub = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            match sub.as_str() {
                "show" => CommandResult::FriendsPanelShow,
                "hide" => CommandResult::FriendsPanelHide,
                _ => CommandResult::StatusMessage("Usage: :friends-panel show|hide".to_string()),
            }
        }
        "channels" => CommandResult::FocusChannels,
        "messages" => CommandResult::FocusMessages,
        "users" => CommandResult::FocusUsers,
        "friends" => CommandResult::FocusFriends,
        "add-friend" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            if nick.is_empty() {
                CommandResult::StatusMessage("Usage: :add-friend <nick>".to_string())
            } else {
                CommandResult::AddFriend(nick)
            }
        }
        "remove-friend" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            if nick.is_empty() {
                CommandResult::StatusMessage("Usage: :remove-friend <nick>".to_string())
            } else {
                CommandResult::RemoveFriend(nick)
            }
        }
        "version" => CommandResult::Version,
        "credits" => CommandResult::Credits,
        "license" => CommandResult::License,
        "whois" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            CommandResult::Whois(nick)
        }
        "secure" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            CommandResult::Secure(nick)
        }
        "unsecure" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            CommandResult::Unsecure(nick)
        }
        "verify" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            CommandResult::Verify(nick)
        }
        "verified" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            CommandResult::Verified(nick)
        }
        "sendfile" => {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let nick = parts.next().unwrap_or("").to_string();
            let path = parts.next().unwrap_or("").trim().to_string();
            CommandResult::SendFile { nick, path }
        }
        "clear" => CommandResult::Clear,
        "search" => CommandResult::Search,
        "highlight" => CommandResult::Highlight,
        "away" => {
            let msg = rest.trim();
            if msg.is_empty() {
                CommandResult::Away(None)
            } else {
                CommandResult::Away(Some(msg.to_string()))
            }
        }
        "notifications" => {
            let sub = rest.split_whitespace().next().unwrap_or("").to_lowercase();
            match sub.as_str() {
                "on" => CommandResult::NotificationsOn,
                "off" => CommandResult::NotificationsOff,
                _ => CommandResult::StatusMessage("Usage: :notifications on|off".to_string()),
            }
        }
        "ignore" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            if nick.is_empty() {
                CommandResult::StatusMessage("Usage: :ignore <nick>".to_string())
            } else {
                CommandResult::Ignore(nick)
            }
        }
        "unignore" => {
            let nick = rest.split_whitespace().next().unwrap_or("").to_string();
            if nick.is_empty() {
                CommandResult::StatusMessage("Usage: :unignore <nick>".to_string())
            } else {
                CommandResult::Unignore(nick)
            }
        }
        "mute" => CommandResult::Mute,
        "unmute" => CommandResult::Unmute,
        "debug-typing" => CommandResult::DebugTyping,
        _ => CommandResult::Unknown(format!("Unknown command: {}", cmd)),
    }
}
