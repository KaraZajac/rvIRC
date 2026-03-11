//! rvIRC - Rust + VIM + IRC. Terminal IRC client with vim-style commands.

mod app;
mod commands;
mod config;
mod connection;
mod events;
mod ui;

use app::{App, Mode, PanelFocus, UserAction};
use config::RvConfig;
use connection::{connect, run_stream, IrcMessage, IrcMessageTx};
use events::{handle_key, KeyAction};
use irc::client::prelude::*;
use irc::proto::Command as IrcCommand;
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
                            if !app.channel_list.contains(ch) {
                                app.channel_list.push(ch.clone());
                            }
                        }
                        if let Some(first) = channels.first() {
                            app.current_channel = Some(first.clone());
                            app.sync_channel_index_to_current();
                        }
                        if !channels.is_empty() {
                            app.status_message = format!("Joined {} channel(s).", channels.len());
                        }
                    }
                }
            }
        }

        // Poll key with short timeout so we can process IRC messages
        let event = crossterm::event::poll(std::time::Duration::from_millis(50));
        let key_action = if let Ok(true) = event {
            crossterm::event::read()
                .ok()
                .and_then(|ev| handle_key(ev, app.mode, app.panel_focus, app.channel_panel_visible, app.user_panel_visible, app.user_action_menu, app.channel_list_popup_visible, app.channel_list_scroll_mode, app.server_list_popup_visible, app.whois_popup_visible))
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
        M::Connected { server } => {
            app.current_server = Some(server);
            app.status_message = "Connected.".to_string();
            app.pending_auto_join = true;
        }
        M::Disconnected => {
            app.current_server = None;
            app.current_channel = None;
            app.current_nickname = None;
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
            app.status_message = "Disconnected.".to_string();
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
                    UserAction::Kick | UserAction::Ban | UserAction::Mute => {
                        app.status_message = format!("{:?} {} - not implemented yet", action, nick);
                        app.user_action_menu = false;
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
                    if !app.channel_list.contains(&ch) {
                        app.channel_list.push(ch.clone());
                    }
                    app.current_channel = Some(ch.clone());
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
        Enter => {
            if app.mode == Mode::Insert {
                let text = app.input.clone();
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
                    }
                } else {
                    app.status_message = "Not connected.".to_string();
                }
            } else if app.mode == Mode::Command {
                let line = app.input.clone();
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
        R::Join(ch) => {
            if let Some(ref c) = client {
                c.send_join(&ch).map_err(|e| e.to_string())?;
                app.channel_list.push(ch.clone());
                app.current_channel = Some(ch.clone());
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
        R::Connect(name) => {
            match config.server_by_name(&name) {
                None => {
                    app.status_message = format!("Unknown server: {}", name);
                }
                Some(server) => {
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
            if let Some(ref c) = client {
                let _ = c.send_quit("");
            }
            if let Some(h) = stream_handle.take() {
                h.abort();
            }
            drop(client.take());
            app.current_server = None;
            app.current_channel = None;
            app.channel_list.clear();
            app.user_list.clear();
            app.status_message = "Disconnected.".to_string();
            return Ok(true);
        }
        R::Msg { nick, text } => {
            if let Some(ref c) = client {
                if !text.is_empty() {
                    c.send_privmsg(&nick, &text).map_err(|e| e.to_string())?;
                }
                if !app.dm_targets.contains(&nick) {
                    app.dm_targets.push(nick.clone());
                }
                app.status_message = format!("Message sent to {}", nick);
            }
        }
        R::SwitchChannel(ch) => {
            app.current_channel = Some(ch);
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
