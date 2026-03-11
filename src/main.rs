//! rvIRC - Rust + VIM + IRC. Terminal IRC client with vim-style commands.

mod app;
mod commands;
mod config;
mod connection;
mod events;
mod ui;

use app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use config::RvConfig;
use connection::{connect, run_stream, IrcMessage, IrcMessageTx};
use events::{handle_key, KeyAction};
use irc::client::prelude::*;
use irc::proto::Command as IrcCommand;
use irc::proto::{ChannelMode as IrcChannelMode, Mode as IrcMode};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use tokio::sync::mpsc;

fn main() -> Result<(), String> {
    let config = RvConfig::load()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    let (irc_tx, mut irc_rx) = mpsc::unbounded_channel::<IrcMessage>();

    let mut app = App::new();
    app.status_message = "Type :connect <server> to connect. :join #channel to join.".to_string();

    let mut client: Option<Client> = None;
    let mut stream_handle: Option<tokio::task::JoinHandle<()>> = None;

    let mut terminal = setup_terminal().map_err(|e| e.to_string())?;
    let mut auto_connect_attempted = false;

    loop {
        terminal.draw(|f| ui::draw(f, &app)).map_err(|e| e.to_string())?;

        // Auto-connect once on startup if a server has auto_connect = "yes"
        if !auto_connect_attempted && client.is_none() {
            auto_connect_attempted = true;
            if let Some(server) = config.servers.iter().find(|s| s.is_auto_connect()) {
                match connect(server, &config, irc_tx.clone(), &rt) {
                    Ok((c, stream)) => {
                        let tx = irc_tx.clone();
                        let handle = rt.spawn(async move {
                            run_stream(stream, tx).await;
                        });
                        stream_handle = Some(handle);
                        client = Some(c);
                        app.current_nickname = config.nickname.clone();
                        app.current_channel = Some("*server*".to_string());
                        app.mark_target_read("*server*");
                        app.channel_index = 0;
                        app.status_message = format!("Auto-connecting to {}...", server.name);
                        if let Some(ref pw) = server.identify_password {
                            if let Some(ref c) = client {
                                let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                            }
                            app.auto_join_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                        } else {
                            app.auto_join_after = None;
                        }
                    }
                    Err(e) => {
                        app.status_message = e;
                    }
                }
            }
        }

        // Drain IRC messages (non-blocking)
        while let Ok(msg) = irc_rx.try_recv() {
            use connection::IrcMessage as M;
            match &msg {
                M::CtcpRequest { from_nick, tag, data, .. } => {
                    if let Some(ref c) = client {
                        let reply = match tag.as_str() {
                            "VERSION" => "VERSION rvIRC 0.1".to_string(),
                            "PING" => data.clone(),
                            "TIME" => {
                                let secs = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap()
                                    .as_secs();
                                format!("TIME {}", secs)
                            }
                            _ => String::new(),
                        };
                        if !reply.is_empty() {
                            let _ = c.send_notice(from_nick, format!("\x01{} {}\x01", tag, reply));
                        }
                    }
                }
                M::NickInUse => {
                    if let Some(ref c) = client {
                        if let Some(ref alt) = config.alt_nick {
                            let _ = c.send(IrcCommand::NICK(alt.clone()));
                            app.status_message = format!("Nick in use, trying {}...", alt);
                        } else {
                            app.status_message = "Nickname in use.".to_string();
                        }
                    }
                }
                _ => {}
            }
            apply_irc_message(&mut app, msg);
        }

        // Auto-join channels after connect: identify first, then join (delay when we identified)
        if app.pending_auto_join {
            let can_join = app.auto_join_after.map_or(true, |t| std::time::Instant::now() >= t);
            if can_join {
                app.pending_auto_join = false;
                app.auto_join_after = None;
                if let (Some(ref c), Some(ref server_name)) = (client.as_ref(), app.current_server.as_ref()) {
                    if let Some(server) = config.server_by_name(server_name) {
                        let channels = server.auto_join_channels();
                        for ch in &channels {
                            let _ = c.send_join(ch);
                            let _ = c.send_topic(ch, "");
                            if !app.channel_list.contains(ch) {
                                app.channel_list.push(ch.clone());
                            }
                        }
                        if let Some(first) = channels.first() {
                            app.current_channel = Some(first.clone());
                            app.mark_target_read(first);
                            app.sync_channel_index_to_current();
                        }
                        if !channels.is_empty() {
                            app.status_message = format!("Joined {} channel(s).", channels.len());
                        }
                    }
                }
            }
        }

        // Auto-reconnect: 3 attempts at 5s, 15s, 30s after disconnect
        if client.is_none()
            && app.reconnect_after.is_some()
            && std::time::Instant::now() >= app.reconnect_after.unwrap()
        {
            let server_name = app.reconnect_server.clone();
            app.reconnect_after = None;
            if let Some(server_name) = server_name {
                if let Some(server) = config.server_by_name(&server_name) {
                    app.status_message = format!("Reconnecting to {} (attempt {})...", server_name, app.reconnect_attempt);
                    match connect(server, &config, irc_tx.clone(), &rt) {
                        Ok((c, stream)) => {
                            let tx = irc_tx.clone();
                            let handle = rt.spawn(async move { run_stream(stream, tx).await });
                            stream_handle = Some(handle);
                            client = Some(c);
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read("*server*");
                            app.channel_index = 0;
                            app.clear_reconnect();
                            if let Some(ref pw) = server.identify_password {
                                if let Some(ref c) = client {
                                    let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                    app.status_message = "Identifying with NickServ...".to_string();
                                }
                                app.auto_join_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                            } else {
                                app.auto_join_after = None;
                            }
                            app.status_message = format!("Reconnected to {}.", server_name);
                        }
                        Err(e) => {
                            if app.reconnect_attempt < 3 {
                                app.reconnect_attempt += 1;
                                let delay_secs = if app.reconnect_attempt == 2 { 15 } else { 30 };
                                app.reconnect_after =
                                    Some(std::time::Instant::now() + std::time::Duration::from_secs(delay_secs));
                                app.reconnect_server = Some(server_name);
                                app.status_message = format!("Reconnect failed: {}. Retry in {}s.", e, delay_secs);
                            } else {
                                app.clear_reconnect();
                                app.status_message = format!("Reconnect failed after 3 attempts: {}", e);
                            }
                        }
                    }
                } else {
                    app.clear_reconnect();
                }
            } else {
                app.clear_reconnect();
            }
        }

        // Poll key with short timeout so we can process IRC messages
        let event = crossterm::event::poll(std::time::Duration::from_millis(50));
        let key_action = if let Ok(true) = event {
            crossterm::event::read()
                .ok()
                .and_then(|ev| handle_key(ev, app.mode, app.panel_focus, app.channel_panel_visible, app.user_panel_visible, app.user_action_menu, app.channel_list_popup_visible, app.channel_list_scroll_mode, app.server_list_popup_visible, app.whois_popup_visible, app.credits_popup_visible, app.license_popup_visible))
        } else {
            None
        };

        if let Some(action) = key_action {
            if matches!(action, KeyAction::QuitApp) {
                break;
            }
            let quit = handle_key_action(
                &mut app,
                &config,
                &mut client,
                &mut stream_handle,
                &irc_tx,
                &rt,
                action,
            )?;
            if quit {
                break;
            }
        }
    }

    restore_terminal().map_err(|e| e.to_string())?;
    Ok(())
}

fn apply_irc_message(app: &mut App, msg: IrcMessage) {
    use connection::IrcMessage as M;
    match msg {
        M::Line { target, line } => {
            if target == app.current_nickname.as_deref().unwrap_or("") {
                app.push_message(&line.source, line.clone());
                if !app.dm_targets.contains(&line.source) {
                    app.dm_targets.push(line.source.clone());
                }
            } else {
                app.push_message(&target, line);
            }
        }
        M::JoinedChannel(ch) => {
            if !app.channel_list.contains(&ch) {
                app.channel_list.push(ch.clone());
            }
        }
        M::PartedChannel(ch) => {
            app.channel_list.retain(|c| c != &ch);
            app.clamp_channel_index();
            if app.current_channel.as_deref() == Some(ch.as_str()) {
                app.current_channel = app.selected_target();
                if let Some(t) = app.current_channel.clone() {
                    app.mark_target_read(&t);
                }
            }
        }
        M::UserList { channel, users } => {
            if app.current_channel.as_deref() == Some(channel.as_str()) {
                app.set_user_list(users);
            }
        }
        M::ChannelList(channels) => {
            let mut list = channels;
            list.sort_by(|a, b| b.1.unwrap_or(0).cmp(&a.1.unwrap_or(0)));
            app.server_channel_list = list;
            app.clamp_channel_list_selected_index();
            app.status_message = format!("{} channels", app.server_channel_list.len());
        }
        M::WhoisResult { nick, lines } => {
            app.whois_nick = nick;
            app.whois_lines = lines;
            app.whois_popup_visible = true;
        }
        M::Topic { channel, topic } => {
            app.channel_topics.insert(channel.clone(), topic.unwrap_or_default());
        }
        M::ChannelModes { channel, modes } => {
            app.channel_modes.insert(channel, modes);
        }
        M::Invite { nick, channel } => {
            app.last_invite = Some((nick.clone(), channel.clone()));
            app.status_message = format!("{} invited you to {} (use :join {} to join)", nick, channel, channel);
        }
        M::NickInUse | M::CtcpRequest { .. } => {}
        M::Connected { server } => {
            app.current_server = Some(server);
            app.status_message = "Connected.".to_string();
            app.pending_auto_join = true;
        }
        M::Disconnected => {
            let server_for_reconnect = app.current_server.clone();
            app.current_server = None;
            app.current_channel = None;
            app.current_nickname = None;
            app.unread_targets.clear();
            app.unread_mentions.clear();
            app.channel_list.clear();
            app.dm_targets.clear();
            app.user_list.clear();
            app.channel_list_popup_visible = false;
            app.server_channel_list.clear();
            app.channel_list_filter.clear();
            app.channel_list_scroll_mode = false;
            app.server_list_popup_visible = false;
            app.whois_popup_visible = false;
            app.whois_nick.clear();
            app.whois_lines.clear();
            app.pending_auto_join = false;
            app.auto_join_after = None;
            app.channel_topics.clear();
            app.channel_modes.clear();
            app.last_invite = None;
            app.status_message = "Disconnected.".to_string();
            if let Some(server) = server_for_reconnect {
                app.reconnect_server = Some(server);
                app.reconnect_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                app.reconnect_attempt = 1;
            }
        }
    }
}

fn handle_key_action(
    app: &mut App,
    config: &RvConfig,
    client: &mut Option<Client>,
    stream_handle: &mut Option<tokio::task::JoinHandle<()>>,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
    action: KeyAction,
) -> Result<bool, String> {
    use KeyAction::*;
    match action {
        NoOp => {}
        QuitApp => return Ok(true),
        SwitchMode(mode) => {
            app.mode = mode;
            if mode == Mode::Command {
                app.input = String::new();
                app.input_cursor = 0;
            }
        }
        FocusChannels => {
            if app.channel_panel_visible {
                app.panel_focus = PanelFocus::Channels;
            }
        }
        FocusUsers => {
            if app.user_panel_visible {
                app.panel_focus = PanelFocus::Users;
                if app.current_channel.as_ref().map_or(false, |t| t.starts_with('#') || t.starts_with('&')) {
                    request_channel_names(client, app);
                }
            }
        }
        UnfocusPanel => app.panel_focus = PanelFocus::Main,
        ChannelUp => {
            app.channel_index = app.channel_index.saturating_sub(1);
        }
        ChannelDown => {
            let len = app.target_list().len();
            if app.channel_index + 1 < len {
                app.channel_index += 1;
            }
        }
        ChannelSelect => {
            if let Some(target) = app.selected_channel() {
                app.current_channel = Some(target.clone());
                app.mark_target_read(&target);
                app.user_list.clear();
                app.message_scroll_offset = 0;
                if target.starts_with('#') || target.starts_with('&') {
                    request_channel_names(client, app);
                }
                app.panel_focus = PanelFocus::Main;
            }
        }
        UserUp => {
            app.user_index = app.user_index.saturating_sub(1);
        }
        UserDown => {
            if app.user_index + 1 < app.user_list.len() {
                app.user_index += 1;
            }
        }
        UserSelect => {
            app.user_action_menu = true;
            app.user_action_index = 0;
        }
        UserActionMenuUp => {
            let n = App::user_actions().len();
            app.user_action_index = app.user_action_index.saturating_sub(1);
            if n > 0 && app.user_action_index >= n {
                app.user_action_index = n - 1;
            }
        }
        UserActionMenuDown => {
            let n = App::user_actions().len();
            if app.user_action_index + 1 < n {
                app.user_action_index += 1;
            }
        }
        UserActionConfirm => {
            let nick = app.selected_user().unwrap_or_default();
            let actions = App::user_actions();
            let idx = app.user_action_index.min(actions.len().saturating_sub(1));
            if let Some(action) = actions.get(idx) {
                match action {
                    UserAction::Dm => {
                        app.user_action_menu = false;
                        app.mode = Mode::Command;
                        app.input = format!("msg {} ", nick);
                    }
                    UserAction::Whois => {
                        app.user_action_menu = false;
                        if let Some(ref c) = client {
                            let _ = c.send(IrcCommand::WHOIS(None, nick.to_string()));
                            app.whois_popup_visible = true;
                            app.whois_nick = nick.to_string();
                            app.whois_lines = vec!["Requesting whois...".to_string()];
                        } else {
                            app.status_message = "Not connected.".to_string();
                        }
                    }
                    UserAction::Kick => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && client.as_ref().is_some() {
                                if let Some(ref c) = client {
                                    let _ = c.send_kick(ch, &nick, "");
                                    app.status_message = format!("Kicked {} from {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Ban => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && client.as_ref().is_some() {
                                if let Some(ref c) = client {
                                    let mask = format!("{}!*@*", nick);
                                    let _ = c.send_mode(ch, &[IrcMode::Plus(IrcChannelMode::Ban, Some(mask))]);
                                    app.status_message = format!("Ban set on {} for {}", ch, nick);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Mute => {
                        app.user_action_menu = false;
                        let key = app.current_channel.as_deref().unwrap_or("*").to_string();
                        app.muted_nicks.entry(key).or_default().insert(nick.clone());
                        app.status_message = format!("Muted {} (local)", nick);
                    }
                }
            }
        }
        CloseUserActionMenu => app.user_action_menu = false,
        ListPopupUp => {
            app.channel_list_selected_index = app.channel_list_selected_index.saturating_sub(1);
        }
        ListPopupDown => {
            let len = app.filtered_server_channel_list().len();
            if app.channel_list_selected_index + 1 < len {
                app.channel_list_selected_index += 1;
            }
        }
        ListPopupSelect => {
            if let Some(ch) = app.selected_list_channel() {
                app.channel_list_popup_visible = false;
                app.channel_list_filter.clear();
                app.channel_list_scroll_mode = false;
                if let Some(ref c) = client {
                    c.send_join(&ch).map_err(|e| e.to_string())?;
                    let _ = c.send_topic(&ch, "");
                    if !app.channel_list.contains(&ch) {
                        app.channel_list.push(ch.clone());
                    }
                    app.current_channel = Some(ch.clone());
                    app.mark_target_read(&ch);
                    app.sync_channel_index_to_current();
                    app.message_scroll_offset = 0;
                    app.status_message = format!("Joined {}", ch);
                }
            }
        }
        ListPopupClose => {
            app.channel_list_popup_visible = false;
            app.channel_list_filter.clear();
            app.channel_list_scroll_mode = false;
        }
        ListPopupFocusList => {
            app.channel_list_scroll_mode = true;
        }
        ListPopupFocusFilter => {
            app.channel_list_scroll_mode = false;
        }
        CloseWhoisPopup => {
            app.whois_popup_visible = false;
        }
        CloseCreditsPopup => {
            app.credits_popup_visible = false;
        }
        CloseLicensePopup => {
            app.license_popup_visible = false;
            app.license_popup_scroll_offset = 0;
        }
        LicenseScrollUp => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_add(1);
        }
        LicenseScrollDown => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_sub(1);
        }
        LicenseScrollPageUp => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_add(15);
        }
        LicenseScrollPageDown => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_sub(15);
        }
        ServerListPopupClose => {
            app.server_list_popup_visible = false;
        }
        ServerListPopupUp => {
            app.server_list_selected_index = app.server_list_selected_index.saturating_sub(1);
        }
        ServerListPopupDown => {
            if app.server_list_selected_index + 1 < app.server_list.len() {
                app.server_list_selected_index += 1;
            }
        }
        ServerListPopupSelect => {
            if let Some(name) = app.selected_server_name() {
                app.server_list_popup_visible = false;
                if let Some(server) = config.server_by_name(&name) {
                    app.clear_reconnect();
                    if let Some(h) = stream_handle.take() {
                        h.abort();
                    }
                    drop(client.take());
                    match connect(server, config, irc_tx.clone(), rt) {
                        Ok((c, stream)) => {
                            let tx = irc_tx.clone();
                            let handle = rt.spawn(async move {
                                run_stream(stream, tx).await;
                            });
                            *stream_handle = Some(handle);
                            *client = Some(c);
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read("*server*");
                            app.channel_index = 0;
                            if let Some(ref pw) = server.identify_password {
                                if let Some(ref c) = client {
                                    let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                    app.status_message = "Identifying with NickServ...".to_string();
                                }
                                app.auto_join_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                            } else {
                                app.auto_join_after = None;
                                app.status_message = format!("Connected to {}.", name);
                            }
                        }
                        Err(e) => {
                            app.status_message = e;
                        }
                    }
                }
            }
        }
        MessageScrollUp => {
            app.message_scroll_offset = app.message_scroll_offset.saturating_add(1);
        }
        MessageScrollDown => {
            app.message_scroll_offset = app.message_scroll_offset.saturating_sub(1);
        }
        MessageScrollPageUp => {
            app.message_scroll_offset = app.message_scroll_offset.saturating_add(15);
        }
        MessageScrollPageDown => {
            app.message_scroll_offset = app.message_scroll_offset.saturating_sub(15);
        }
        ListPopupFilterChar(c) => {
            if c != '\0' {
                app.channel_list_filter.push(c);
                app.clamp_channel_list_selected_index();
            }
        }
        ListPopupBackspace => {
            if !app.channel_list_filter.is_empty() {
                app.channel_list_filter.pop();
                app.clamp_channel_list_selected_index();
            }
        }
        Char(c) => {
            if c != '\0' {
                if app.mode == Mode::Insert || app.mode == Mode::Command {
                    app.input.push(c);
                    app.input_cursor = app.input.len();
                }
            }
        }
        Backspace => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                let len = app.input.len();
                app.input_cursor = app.input_cursor.min(len);
                if app.input_cursor > 0 {
                    app.input.remove(app.input_cursor - 1);
                    app.input_cursor -= 1;
                }
            }
        }
        InputHistoryUp => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                if app.input_history_index == 0 {
                    app.input_draft = app.input.clone();
                }
                if app.input_history_index < app.input_history.len() {
                    app.input_history_index += 1;
                    let idx = app.input_history_index - 1;
                    app.input = app.input_history[idx].clone();
                    app.input_cursor = app.input.len();
                }
            }
        }
        InputHistoryDown => {
            if (app.mode == Mode::Insert || app.mode == Mode::Command) && app.input_history_index > 0 {
                app.input_history_index -= 1;
                if app.input_history_index == 0 {
                    app.input = app.input_draft.clone();
                } else {
                    app.input = app.input_history[app.input_history_index - 1].clone();
                }
                app.input_cursor = app.input.len();
            }
        }
        TabComplete => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                complete_input(app);
            }
        }
        Enter => {
            if app.mode == Mode::Insert {
                let text = app.input.clone();
                app.input_history_index = 0;
                app.input_draft.clear();
                if !text.is_empty() && app.input_history.first().as_deref() != Some(&text) {
                    app.input_history.insert(0, text.clone());
                    if app.input_history.len() > 100 {
                        app.input_history.pop();
                    }
                }
                app.input.clear();
                app.input_cursor = 0;
                if text.starts_with(':') {
                    if run_command(app, client, stream_handle, config, irc_tx, rt, &text)? {
                        return Ok(true);
                    }
                } else if let Some(ref c) = client {
                    let target = app.current_channel.as_deref().unwrap_or("*").to_string();
                    if target == "*server*" {
                        app.status_message = "Cannot send to server.".to_string();
                    } else if !text.is_empty() {
                        c.send_privmsg(&target, &text).map_err(|e| e.to_string())?;
                        let nick = app.current_nickname.clone().unwrap_or_else(|| "?".to_string());
                        app.push_message(&target, MessageLine { source: nick, text, kind: MessageKind::Privmsg });
                    }
                } else {
                    app.status_message = "Not connected.".to_string();
                }
            } else if app.mode == Mode::Command {
                let line = app.input.clone();
                app.input_history_index = 0;
                app.input_draft.clear();
                if !line.is_empty() && app.input_history.first().as_deref() != Some(&line) {
                    app.input_history.insert(0, line.clone());
                    if app.input_history.len() > 100 {
                        app.input_history.pop();
                    }
                }
                app.input.clear();
                app.mode = Mode::Normal;
                if run_command(app, client, stream_handle, config, irc_tx, rt, &line)? {
                    return Ok(true);
                }
            }
        }
        Esc => {
            app.mode = Mode::Normal;
            app.input.clear();
            app.input_cursor = 0;
            app.user_action_menu = false;
            app.panel_focus = PanelFocus::Main;
        }
    }
    Ok(false)
}

/// Returns Ok(true) if the program should exit (e.g. after :quit / :q).
fn run_command(
    app: &mut App,
    client: &mut Option<Client>,
    stream_handle: &mut Option<tokio::task::JoinHandle<()>>,
    config: &RvConfig,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
    line: &str,
) -> Result<bool, String> {
    let line = line.trim_start_matches(':');
    let result = commands::parse(line);
    use commands::CommandResult as R;
    match result {
        R::Join { channel: ch, key } => {
            if let Some(ref c) = client {
                if let Some(ref k) = key {
                    c.send_join_with_keys(&ch, k).map_err(|e| e.to_string())?;
                } else {
                    c.send_join(&ch).map_err(|e| e.to_string())?;
                }
                let _ = c.send_topic(&ch, "");
                app.channel_list.push(ch.clone());
                app.current_channel = Some(ch.clone());
                app.mark_target_read(&ch);
                app.sync_channel_index_to_current();
                app.message_scroll_offset = 0;
                app.status_message = format!("Joined {}", ch);
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Part(Some(ch)) => {
            if let Some(ref c) = client {
                c.send_part(&ch).map_err(|e| e.to_string())?;
                app.channel_list.retain(|x| x != &ch);
                app.clamp_channel_index();
                app.current_channel = app.selected_target();
                if let Some(t) = app.current_channel.clone() {
                    app.mark_target_read(&t);
                }
            }
        }
        R::Part(None) => {
            if let Some(ref ch) = app.current_channel {
                if (ch.starts_with('#') || ch.starts_with('&')) && app.channel_list.iter().any(|c| c == ch) {
                    if let Some(ref c) = client {
                        c.send_part(ch).map_err(|e| e.to_string())?;
                        app.channel_list.retain(|x| x != ch);
                    }
                }
                app.clamp_channel_index();
                app.current_channel = app.selected_target();
                if let Some(t) = app.current_channel.clone() {
                    app.mark_target_read(&t);
                }
            }
        }
        R::List => {
            if let Some(ref c) = client {
                let _ = c.send(IrcCommand::LIST(None, None));
                app.channel_list_popup_visible = true;
                app.server_channel_list = Vec::new();
                app.channel_list_filter.clear();
                app.channel_list_selected_index = 0;
                app.channel_list_scroll_mode = false;
                app.status_message = "Fetching channel list...".to_string();
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Servers => {
            app.server_list = config.servers.iter().map(|s| s.name.clone()).collect();
            app.server_list_popup_visible = true;
            app.server_list_selected_index = 0;
            app.clamp_server_list_selected_index();
        }
        R::Reconnect => {
            if let Some(server_name) = app.current_server.clone() {
                if let Some(server) = config.server_by_name(&server_name) {
                    if let Some(h) = stream_handle.take() {
                        h.abort();
                    }
                    drop(client.take());
                    match connect(server, config, irc_tx.clone(), rt) {
                        Ok((c, stream)) => {
                            let tx = irc_tx.clone();
                            let handle = rt.spawn(async move { run_stream(stream, tx).await });
                            *stream_handle = Some(handle);
                            *client = Some(c);
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read("*server*");
                            app.channel_index = 0;
                            if let Some(ref pw) = server.identify_password {
                                if let Some(ref c) = client {
                                    let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                    app.status_message = "Identifying with NickServ...".to_string();
                                }
                                app.auto_join_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                            } else {
                                app.auto_join_after = None;
                            }
                            app.status_message = format!("Reconnected to {}.", &server_name);
                        }
                        Err(e) => app.status_message = e,
                    }
                } else {
                    app.status_message = "Unknown server.".to_string();
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Connect(name) => {
            match config.server_by_name(&name) {
                None => {
                    app.status_message = format!("Unknown server: {}", name);
                }
                Some(server) => {
                    app.clear_reconnect();
                    if let Some(h) = stream_handle.take() {
                        h.abort();
                    }
                    drop(client.take());
                    match connect(server, config, irc_tx.clone(), rt) {
                        Ok((c, stream)) => {
                            let tx = irc_tx.clone();
                            let handle = rt.spawn(async move {
                                run_stream(stream, tx).await;
                            });
                            *stream_handle = Some(handle);
                            *client = Some(c);
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read("*server*");
                            app.channel_index = 0;
                            if let Some(ref pw) = server.identify_password {
                                if let Some(ref c) = client {
                                    let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                    app.status_message = "Identifying with NickServ...".to_string();
                                }
                                app.auto_join_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                            } else {
                                app.auto_join_after = None;
                            }
                        }
                        Err(e) => {
                            app.status_message = e;
                        }
                    }
                }
            }
        }
        R::Quit(_) => {
            app.clear_reconnect();
            if let Some(ref c) = client {
                let _ = c.send_quit("");
            }
            if let Some(h) = stream_handle.take() {
                h.abort();
            }
            drop(client.take());
            app.current_server = None;
            app.current_channel = None;
            app.unread_targets.clear();
            app.unread_mentions.clear();
            app.channel_list.clear();
            app.user_list.clear();
            app.status_message = "Disconnected.".to_string();
            return Ok(true);
        }
        R::Msg { nick, text } => {
            if let Some(ref c) = client {
                if !text.is_empty() {
                    c.send_privmsg(&nick, &text).map_err(|e| e.to_string())?;
                    let our_nick = app.current_nickname.clone().unwrap_or_else(|| "?".to_string());
                    app.push_message(&nick, MessageLine { source: our_nick, text: text.clone(), kind: MessageKind::Privmsg });
                }
                if !app.dm_targets.contains(&nick) {
                    app.dm_targets.push(nick.clone());
                }
                app.status_message = format!("Message sent to {}", nick);
            }
        }
        R::Me(text) => {
            if let (Some(ref c), Some(ref target)) = (client.as_ref(), app.current_channel.as_ref()) {
                if target.as_str() == "*server*" {
                    app.status_message = "Cannot /me to server.".to_string();
                } else if !text.is_empty() {
                    c.send_action(target, &text).map_err(|e| e.to_string())?;
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Nick(newnick) => {
            if let Some(ref c) = client {
                let _ = c.send(IrcCommand::NICK(newnick.clone()));
                app.current_nickname = Some(newnick.clone());
                app.status_message = format!("Changing nick to {}", newnick);
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Topic(Some(topic)) => {
            if let (Some(ref c), Some(ref ch)) = (client.as_ref(), app.current_channel.as_ref()) {
                if ch.starts_with('#') || ch.starts_with('&') {
                    c.send_topic(ch, &topic).map_err(|e| e.to_string())?;
                    app.channel_topics.insert(ch.to_string(), topic.clone());
                    app.status_message = "Topic set.".to_string();
                } else {
                    app.status_message = "Not a channel.".to_string();
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Topic(None) => {
            if let Some(ref ch) = app.current_channel.as_ref() {
                if ch.starts_with('#') || ch.starts_with('&') {
                    if let Some(ref c) = client {
                        let _ = c.send_topic(ch, "");
                    }
                    if let Some(t) = app.channel_topics.get(ch.as_str()) {
                        app.status_message = if t.is_empty() { "No topic set.".to_string() } else { t.clone() };
                    } else {
                        app.status_message = "Requesting topic...".to_string();
                    }
                }
            }
        }
        R::Kick { channel, nick, reason } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some(ref c), Some(ch)) = (client.as_ref(), ch) {
                c.send_kick(&ch, &nick, reason.as_deref().unwrap_or("")).map_err(|e| e.to_string())?;
                app.status_message = format!("Kicked {} from {}", nick, ch);
            } else {
                app.status_message = "Usage: :kick [channel] <nick> [reason]".to_string();
            }
        }
        R::Ban { channel, mask } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some(ref c), Some(ch)) = (client.as_ref(), ch) {
                if mask.is_empty() {
                    app.status_message = "Usage: :ban [channel] <mask>".to_string();
                } else {
                    c.send_mode(&ch, &[IrcMode::Plus(IrcChannelMode::Ban, Some(mask))]).map_err(|e| e.to_string())?;
                    app.status_message = format!("Ban set on {}", ch);
                }
            } else {
                app.status_message = "Usage: :ban [channel] <mask>".to_string();
            }
        }
        R::SwitchChannel(ch) => {
            app.current_channel = Some(ch.clone());
            app.mark_target_read(&ch);
            app.sync_channel_index_to_current();
            app.message_scroll_offset = 0;
        }
        R::StatusMessage(m) => app.status_message = m,
        R::ChannelPanelShow => app.channel_panel_visible = true,
        R::ChannelPanelHide => {
            app.channel_panel_visible = false;
            if app.panel_focus == PanelFocus::Channels {
                app.panel_focus = PanelFocus::Main;
            }
        }
        R::UserPanelShow => app.user_panel_visible = true,
        R::UserPanelHide => {
            app.user_panel_visible = false;
            if app.panel_focus == PanelFocus::Users {
                app.panel_focus = PanelFocus::Main;
            }
        }
        R::FocusChannels => {
            if app.channel_panel_visible {
                app.panel_focus = PanelFocus::Channels;
            }
        }
        R::Version => {
            app.status_message = "rvIRC 1.0.0".to_string();
        }
        R::Credits => {
            app.credits_popup_visible = true;
        }
        R::License => {
            app.license_popup_visible = true;
        }
        R::FocusUsers => {
            if app.user_panel_visible {
                app.panel_focus = PanelFocus::Users;
                request_channel_names(client, app);
            }
        }
        R::SendPrivmsg { target, text } => {
            if let Some(ref c) = client {
                c.send_privmsg(&target, &text).map_err(|e| e.to_string())?;
            }
        }
        R::NoOp => {}
        R::Unknown(m) => app.status_message = m,
        R::UserAction { .. } => {}
    }
    Ok(false)
}

/// Tab completion: command name only (first word after :).
fn complete_input(app: &mut App) {
    const COMMANDS: &[&str] = &[
        "join", "part", "list", "servers", "connect", "reconnect", "quit", "q",
        "msg", "me", "nick", "topic", "kick", "ban", "channel", "chan", "c",
        "channel-panel", "user-panel", "channels", "users",
        "version", "credits", "license",
    ];
    let input = &app.input;
    let cursor = app.input_cursor.min(input.len());
    if !input.starts_with(':') {
        return;
    }
    let rest = &input[1..];
    let first_space = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let first_word = rest[..first_space].to_lowercase();
    if cursor > 1 + first_space {
        return;
    }
    let partial = first_word.as_str();
    let candidates: Vec<&str> = COMMANDS.iter().filter(|c| c.starts_with(partial)).copied().collect();
    if candidates.len() == 1 {
        app.input = format!(":{} ", candidates[0]);
        app.input_cursor = app.input.len();
    } else if candidates.len() > 1 {
        let common = common_prefix(candidates.iter().copied());
        if common != partial {
            app.input = format!(":{}", common);
            app.input_cursor = app.input.len();
        }
    }
}

fn common_prefix(mut it: impl Iterator<Item = impl AsRef<str>>) -> String {
    let first = match it.next() {
        Some(s) => s.as_ref().to_string(),
        None => return String::new(),
    };
    let mut prefix = first;
    for s in it {
        let s = s.as_ref();
        while !prefix.is_empty() && !s.starts_with(&prefix) {
            prefix.pop();
        }
    }
    prefix
}

/// Request NAMES for the current channel so the user list is populated.
fn request_channel_names(client: &mut Option<Client>, app: &App) {
    if let (Some(ref c), Some(ref ch)) = (client.as_ref(), app.current_channel.as_ref()) {
        if ch.starts_with('#') || ch.starts_with('&') {
            let _ = c.send(IrcCommand::NAMES(Some(ch.to_string()), None));
        }
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    Terminal::new(backend)
}

fn restore_terminal() -> io::Result<()> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    Ok(())
}
