//! rvIRC - Rust + VIM + IRC. Terminal IRC client with vim-style commands.

mod app;
mod commands;
mod config;
mod connection;
mod crypto;
mod events;
mod filetransfer;
mod ui;

use app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use config::RvConfig;
use connection::{connect, run_stream, IrcMessage, IrcMessageTx};
use crypto::{KnownKeys, SecureSession, TofuResult, key_fingerprint};
use base64::Engine;
use events::{handle_key, KeyAction};
use irc::client::prelude::*;
use irc::proto::Command as IrcCommand;
use irc::proto::{ChannelMode as IrcChannelMode, Mode as IrcMode};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io;
use std::path::PathBuf;
use tokio::sync::mpsc;

fn main() -> Result<(), String> {
    let config = RvConfig::load()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    let (irc_tx, mut irc_rx) = mpsc::unbounded_channel::<IrcMessage>();

    let mut app = App::new();
    app.render_images = config.render_images;
    app.status_message = "Type :connect <server> to connect. :join #channel to join.".to_string();

    // Load persistent identity keypair (same directory as config.toml)
    if let Some(config_dir) = RvConfig::config_dir() {
        let identity_path = config_dir.join("identity.toml");
        match crypto::Keypair::load_or_generate(&identity_path) {
            Ok(kp) => app.keypair = kp,
            Err(e) => app.status_message = format!("Identity key error: {}", e),
        }
        let known_keys_path = config_dir.join("known_keys.toml");
        app.known_keys = KnownKeys::load(&known_keys_path);
        app.known_keys_path = Some(known_keys_path);
    }

    let mut client: Option<Client> = None;
    let mut stream_handle: Option<tokio::task::JoinHandle<()>> = None;

    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    let mut terminal = setup_terminal().map_err(|e| e.to_string())?;
    let mut auto_connect_attempted = false;

    loop {
        terminal.draw(|f| ui::draw(f, &mut app)).map_err(|e| e.to_string())?;

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
                M::SendPrivmsg { ref target, ref text } => {
                    if let Some(ref c) = client {
                        let _ = c.send_privmsg(target, text);
                    }
                }
                M::Status(ref s) => {
                    app.status_message = s.clone();
                }
                M::ChatLog { ref target, ref text } => {
                    app.push_message(
                        target,
                        MessageLine {
                            source: "***".to_string(),
                            text: text.clone(),
                            kind: MessageKind::Other,
                            image_id: None,
                        },
                    );
                }
                M::ImageReady { image_id, ref image } => {
                    let protocol = picker.new_resize_protocol(image.clone());
                    app.inline_images.insert(*image_id, app::InlineImage::Static(protocol));
                }
                M::AnimatedImageReady { image_id, ref frames, ref delays } => {
                    let encoded: Vec<_> = frames.iter()
                        .map(|f| picker.new_resize_protocol(f.clone()))
                        .collect();
                    app.inline_images.insert(*image_id, app::InlineImage::Animated {
                        frames: encoded,
                        delays: delays.clone(),
                        current_frame: 0,
                        last_advance: std::time::Instant::now(),
                    });
                }
                M::TransferProgress { nick, filename, bytes, total, is_send } => {
                    app.transfer_progress_visible = true;
                    app.transfer_progress_nick = nick.clone();
                    app.transfer_progress_filename = filename.clone();
                    app.transfer_progress_bytes = *bytes;
                    app.transfer_progress_total = *total;
                    app.transfer_progress_is_send = *is_send;
                }
                M::TransferComplete { .. } => {
                    app.transfer_progress_visible = false;
                }
                _ => {}
            }
            apply_irc_message(&mut app, msg, &irc_tx, &rt);
        }

        if !app.protocol_events.is_empty() {
            process_protocol_events(&mut app, &mut client, &rt, &irc_tx);
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
                .and_then(|ev| handle_key(ev, app.mode, app.panel_focus, app.channel_panel_visible, app.user_panel_visible, app.user_action_menu, app.channel_list_popup_visible, app.channel_list_scroll_mode, app.server_list_popup_visible, app.whois_popup_visible, app.credits_popup_visible, app.license_popup_visible, app.file_receive_popup_visible, app.file_browser_visible, app.secure_accept_popup_visible))
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

    // Clean disconnect so the server and other users see a proper QUIT (not just connection closed)
    if let Some(c) = client.take() {
        let _ = c.send_quit("Leaving");
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if let Some(h) = stream_handle.take() {
        h.abort();
    }

    restore_terminal().map_err(|e| e.to_string())?;
    Ok(())
}

fn apply_irc_message(
    app: &mut App,
    msg: IrcMessage,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) {
    use connection::IrcMessage as M;
    match msg {
        M::Line { target, mut line } => {
            if line.text.starts_with("[:rvIRC:") {
                if let Some(evt) = parse_rvirc_protocol(&line.source, &line.text) {
                    app.protocol_events.push(evt);
                }
                if !app.dm_targets.contains(&line.source) {
                    app.dm_targets.push(line.source.clone());
                }
                return;
            }
            if app.render_images {
                if let Some(url) = extract_image_url(&line.text) {
                    let image_id = app.next_image_id;
                    app.next_image_id += 1;
                    line.image_id = Some(image_id);
                    spawn_image_download(url, image_id, irc_tx, rt);
                }
            }
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
        M::NickInUse | M::CtcpRequest { .. } | M::SendPrivmsg { .. } | M::Status(_) | M::ChatLog { .. } | M::ImageReady { .. } | M::AnimatedImageReady { .. } | M::TransferProgress { .. } | M::TransferComplete { .. } => {}
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
            // Up/k = see higher in document = decrease offset
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_sub(1);
        }
        LicenseScrollDown => {
            // Down/j = see lower in document = increase offset (ratatui: higher y = further down)
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_add(1);
        }
        LicenseScrollPageUp => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_sub(15);
        }
        LicenseScrollPageDown => {
            app.license_popup_scroll_offset = app.license_popup_scroll_offset.saturating_add(15);
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
                        if app.secure_sessions.contains_key(&target) {
                            let session = app.secure_sessions.get_mut(&target).unwrap();
                            match session.encrypt(&text) {
                                Ok((nonce, ct)) => {
                                    let wire = format!("[:rvIRC:ENC:{}:{}]", nonce, ct);
                                    c.send_privmsg(&target, &wire).map_err(|e| e.to_string())?;
                                    push_self_message(app, &target, text, irc_tx, rt);
                                }
                                Err(e) => {
                                    app.status_message = format!("Encrypt error: {}", e);
                                }
                            }
                        } else {
                            c.send_privmsg(&target, &text).map_err(|e| e.to_string())?;
                            push_self_message(app, &target, text, irc_tx, rt);
                        }
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
        SecureAccept => {
            let nick = app.secure_accept_nick.clone();
            let ephemeral_b64 = app.secure_accept_ephemeral_b64.clone();
            let identity_b64 = app.secure_accept_identity_b64.clone();
            app.secure_accept_popup_visible = false;

            let their_identity_bytes: [u8; 32] = match base64::engine::general_purpose::STANDARD
                .decode(&identity_b64)
                .ok()
                .and_then(|v| v.try_into().ok())
            {
                Some(b) => b,
                None => {
                    app.push_chat_log(&nick, "Secure handshake failed: invalid identity key.");
                    app.status_message = "Secure handshake failed.".to_string();
                    return Ok(false);
                }
            };

            // TOFU: upsert the peer's identity key
            let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
            app.known_keys.upsert(&nick, &server, &identity_b64);
            if let Some(ref path) = app.known_keys_path {
                let _ = app.known_keys.save(path);
            }

            let our_ephemeral = crypto::Keypair::generate();
            let our_ephemeral_pub_b64 = our_ephemeral.public_key_b64();
            let our_identity_pub_b64 = app.keypair.public_key_b64();

            match SecureSession::from_exchange(
                &our_ephemeral.secret,
                &our_ephemeral.public,
                &ephemeral_b64,
                &app.keypair.public,
                their_identity_bytes,
            ) {
                Ok(session) => {
                    app.secure_sessions.insert(nick.clone(), session);
                    if let Some(ref c) = client {
                        let msg = format!("[:rvIRC:SECURE:ACK:{}:{}]", our_ephemeral_pub_b64, our_identity_pub_b64);
                        let _ = c.send_privmsg(&nick, &msg);
                    }
                    let fp = key_fingerprint(&identity_b64);
                    app.push_chat_log(&nick, &format!("Key fingerprint: {}", fp));
                    app.push_chat_log(&nick, "*** SECURE CONNECTION ESTABLISHED ***");
                    app.push_chat_log(&nick, "Messages are now end-to-end encrypted (X25519 + ChaCha20-Poly1305).");
                    if !app.known_keys.is_verified(&nick, &server) {
                        app.push_chat_log(&nick, "Use :verify to compare verification codes.");
                    }
                    app.status_message = format!("Secure session established with {}.", nick);
                }
                Err(e) => {
                    app.push_chat_log(&nick, &format!("Secure handshake failed: {}", e));
                    app.status_message = format!("Secure handshake from {} failed: {}", nick, e);
                }
            }
        }
        SecureReject => {
            let nick = app.secure_accept_nick.clone();
            app.secure_accept_popup_visible = false;
            app.push_chat_log(&nick, "*** Secure session request rejected. ***");
            app.status_message = format!("Rejected secure session from {}.", nick);
        }
        FileReceiveAccept => {
            let code = app.file_receive_code.clone();
            let filename = app.file_receive_filename.clone();
            let nick = app.file_receive_nick.clone();
            app.file_receive_popup_visible = false;

            app.push_chat_log(&nick, &format!("Accepted file: {}", filename));

            if let Some(dl_dir) = config.resolved_download_dir() {
                let safe_name = sanitize_received_filename(&filename);
                let save_path = dl_dir.join(&safe_name);
                let tx = irc_tx.clone();
                let nick_c = nick.clone();
                app.push_chat_log(&nick, &format!("Saving to {}...", save_path.display()));
                app.status_message = format!("Receiving {} from {}...", filename, nick);
                rt.spawn(async move {
                    match filetransfer::receive_file(&code, &save_path, &nick_c, &tx).await {
                        Ok(()) => {
                            let _ = tx.send(IrcMessage::SendPrivmsg {
                                target: nick_c.clone(),
                                text: "[:rvIRC:WORMHOLE:COMPLETE]".to_string(),
                            });
                            let _ = tx.send(IrcMessage::Status(format!(
                                "File saved to {}",
                                save_path.display()
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(IrcMessage::ChatLog {
                                target: nick_c,
                                text: format!("File receive failed: {}", e),
                            });
                            let _ = tx.send(IrcMessage::Status(format!("File receive failed: {}", e)));
                        }
                    }
                });
            } else {
                app.file_browser_visible = true;
                app.file_browser_pending_filename = filename;
                app.file_browser_pending_code = code;
                app.file_browser_pending_nick = nick;
                app.file_browser_mode = app::FileBrowserMode::ReceiveFile;
                let home = directories::BaseDirs::new()
                    .map(|b| b.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                app.file_browser_path = home;
                app.refresh_file_browser();
            }
        }
        FileReceiveReject => {
            let nick = app.file_receive_nick.clone();
            app.file_receive_popup_visible = false;
            if let Some(ref c) = client {
                let _ = c.send_privmsg(&nick, "[:rvIRC:WORMHOLE:REJECT]");
            }
            app.status_message = format!("Rejected file from {}.", nick);
        }
        FileBrowserUp => {
            app.file_browser_selected_index = app.file_browser_selected_index.saturating_sub(1);
        }
        FileBrowserDown => {
            if app.file_browser_selected_index + 1 < app.file_browser_entries.len() {
                app.file_browser_selected_index += 1;
            }
        }
        FileBrowserEnter => {
            if let Some((name, is_dir)) = app.file_browser_entries.get(app.file_browser_selected_index).cloned() {
                if is_dir {
                    app.file_browser_path = app.file_browser_path.join(&name);
                    app.refresh_file_browser();
                } else if app.file_browser_mode == app::FileBrowserMode::SendFile {
                    let file_path = app.file_browser_path.join(&name);
                    let nick = app.file_browser_pending_nick.clone();
                    app.file_browser_visible = false;

                    let tx = irc_tx.clone();
                    let nick_clone = nick.clone();
                    app.push_chat_log(&nick, &format!("Starting file send: {}", name));
                    app.status_message = format!("Starting file send of {} to {}...", name, nick);
                    rt.spawn(async move {
                        match filetransfer::send_file(&file_path, nick_clone.clone(), tx.clone()).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = tx.send(IrcMessage::ChatLog {
                                    target: nick_clone.clone(),
                                    text: format!("File send failed: {}", e),
                                });
                                let _ = tx.send(IrcMessage::Status(format!(
                                    "File send failed: {}", e
                                )));
                            }
                        }
                    });
                }
            }
        }
        FileBrowserBack => {
            if let Some(parent) = app.file_browser_path.parent().map(|p| p.to_path_buf()) {
                app.file_browser_path = parent;
                app.refresh_file_browser();
            }
        }
        FileBrowserSelect => {
            if app.file_browser_mode == app::FileBrowserMode::ReceiveFile {
                let save_dir = app.file_browser_path.clone();
                let filename = app.file_browser_pending_filename.clone();
                let code = app.file_browser_pending_code.clone();
                let nick = app.file_browser_pending_nick.clone();
                app.file_browser_visible = false;

                let safe_name = sanitize_received_filename(&filename);
                let save_path = save_dir.join(&safe_name);
                let tx = irc_tx.clone();
                let nick_c = nick.clone();
                app.push_chat_log(&nick, &format!("Receiving {} to {}...", filename, save_path.display()));
                app.status_message = format!("Receiving {} from {}...", filename, nick);
                rt.spawn(async move {
                    match filetransfer::receive_file(&code, &save_path, &nick_c, &tx).await {
                        Ok(()) => {
                            let _ = tx.send(IrcMessage::SendPrivmsg {
                                target: nick_c.clone(),
                                text: "[:rvIRC:WORMHOLE:COMPLETE]".to_string(),
                            });
                            let _ = tx.send(IrcMessage::Status(format!(
                                "File saved to {}",
                                save_path.display()
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(IrcMessage::ChatLog {
                                target: nick_c,
                                text: format!("File receive failed: {}", e),
                            });
                            let _ = tx.send(IrcMessage::Status(format!("File receive failed: {}", e)));
                        }
                    }
                });
            }
        }
        FileBrowserClose => {
            app.file_browser_visible = false;
            let nick = app.file_browser_pending_nick.clone();
            if let Some(ref c) = client {
                let _ = c.send_privmsg(&nick, "[:rvIRC:WORMHOLE:REJECT]");
            }
            app.status_message = "File transfer cancelled.".to_string();
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
                let _ = c.send_quit("Leaving");
                std::thread::sleep(std::time::Duration::from_millis(250));
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
                    if app.secure_sessions.contains_key(&nick) {
                        let session = app.secure_sessions.get_mut(&nick).unwrap();
                        match session.encrypt(&text) {
                            Ok((nonce, ct)) => {
                                let wire = format!("[:rvIRC:ENC:{}:{}]", nonce, ct);
                                c.send_privmsg(&nick, &wire).map_err(|e| e.to_string())?;
                                push_self_message(app, &nick, text.clone(), irc_tx, rt);
                            }
                            Err(e) => {
                                app.status_message = format!("Encrypt error: {}", e);
                            }
                        }
                    } else {
                        c.send_privmsg(&nick, &text).map_err(|e| e.to_string())?;
                        push_self_message(app, &nick, text.clone(), irc_tx, rt);
                    }
                }
                if !app.dm_targets.contains(&nick) {
                    app.dm_targets.push(nick.clone());
                }
                app.current_channel = Some(nick.clone());
                app.mark_target_read(&nick);
                app.sync_channel_index_to_current();
                app.message_scroll_offset = 0;
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
        R::Secure(nick_arg) => {
            let nick = if nick_arg.is_empty() {
                app.current_dm_nick().unwrap_or_default()
            } else {
                nick_arg
            };
            if nick.is_empty() {
                app.status_message = "Usage: :secure <nick> (or use in a DM)".to_string();
            } else if let Some(ref c) = client {
                let ephemeral = crypto::Keypair::generate();
                let ephemeral_pub_b64 = ephemeral.public_key_b64();
                let identity_pub_b64 = app.keypair.public_key_b64();
                let msg = format!("[:rvIRC:SECURE:INIT:{}:{}]", ephemeral_pub_b64, identity_pub_b64);
                c.send_privmsg(&nick, &msg).map_err(|e| e.to_string())?;
                app.pending_secure.insert(nick.clone());
                app.pending_secure_ephemeral.insert(nick.clone(), ephemeral);
                if !app.dm_targets.contains(&nick) {
                    app.dm_targets.push(nick.clone());
                }
                app.push_chat_log(&nick, &format!("*** ESTABLISHING SECURE CONNECTION WITH {} ***", nick));
                app.push_chat_log(&nick, "Sending key exchange request...");
                app.status_message = format!("Initiating secure session with {}...", nick);
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Unsecure(nick_arg) => {
            let nick = if nick_arg.is_empty() {
                app.current_dm_nick().unwrap_or_default()
            } else {
                nick_arg
            };
            if nick.is_empty() {
                app.status_message = "Usage: :unsecure <nick> (or use in a DM)".to_string();
            } else if app.secure_sessions.remove(&nick).is_some() {
                app.push_chat_log(&nick, "*** SECURE SESSION ENDED ***");
                app.status_message = format!("Secure session with {} ended.", nick);
            } else {
                app.status_message = format!("No secure session with {}.", nick);
            }
        }
        R::SendFile { nick: nick_arg, path } => {
            let nick = if nick_arg.is_empty() {
                app.current_dm_nick().unwrap_or_default()
            } else {
                nick_arg
            };
            if nick.is_empty() {
                app.status_message = "Usage: :sendfile <nick> <path> (or use in a DM)".to_string();
            } else if client.is_none() {
                app.status_message = "Not connected.".to_string();
            } else if path.is_empty() {
                app.file_browser_visible = true;
                app.file_browser_pending_filename = String::new();
                app.file_browser_pending_code = String::new();
                app.file_browser_pending_nick = nick;
                let home = directories::BaseDirs::new()
                    .map(|b| b.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                app.file_browser_path = home;
                app.file_browser_mode = app::FileBrowserMode::SendFile;
                app.refresh_file_browser();
            } else {
                let file_path = PathBuf::from(&path);
                if !file_path.exists() {
                    app.status_message = format!("File not found: {}", path);
                } else {
                    let tx = irc_tx.clone();
                    let nick_clone = nick.clone();
                    let file_name = file_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    app.push_chat_log(&nick, &format!("Starting file send: {}", file_name));
                    app.status_message = format!("Starting file send of {} to {}...", file_name, nick);
                    rt.spawn(async move {
                        match filetransfer::send_file(&file_path, nick_clone.clone(), tx.clone()).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = tx.send(IrcMessage::ChatLog {
                                    target: nick_clone.clone(),
                                    text: format!("File send failed: {}", e),
                                });
                                let _ = tx.send(IrcMessage::Status(format!(
                                    "File send failed: {}", e
                                )));
                            }
                        }
                    });
                }
            }
        }
        R::Verify(nick_arg) => {
            let nick = if nick_arg.is_empty() {
                app.current_dm_nick().unwrap_or_default()
            } else {
                nick_arg
            };
            if nick.is_empty() {
                app.status_message = "Usage: :verify <nick> (or use in a DM)".to_string();
            } else if let Some(session) = app.secure_sessions.get(&nick) {
                let words = session.sas_words();
                let code = words.join(" ");
                app.push_chat_log(&nick, &format!("*** Verification code with {}: {} ***", nick, code));
                app.push_chat_log(&nick, "Both sides must run :verify -- ask your peer to run it too.");
                app.push_chat_log(&nick, "Compare the 6 words out-of-band (voice, in person, etc). If they match, run :verified");
                app.status_message = format!("SAS: {}", code);
            } else {
                app.status_message = format!("No secure session with {}.", nick);
            }
        }
        R::Verified(nick_arg) => {
            let nick = if nick_arg.is_empty() {
                app.current_dm_nick().unwrap_or_default()
            } else {
                nick_arg
            };
            if nick.is_empty() {
                app.status_message = "Usage: :verified <nick> (or use in a DM)".to_string();
            } else {
                let server = app.current_server.as_deref().unwrap_or("unknown");
                if app.known_keys.set_verified(&nick, server) {
                    if let Some(ref path) = app.known_keys_path {
                        let _ = app.known_keys.save(path);
                    }
                    app.push_chat_log(&nick, &format!("*** {} is now marked as VERIFIED ***", nick));
                    app.status_message = format!("{} marked as verified.", nick);
                } else {
                    app.status_message = format!("No known key for {}.", nick);
                }
            }
        }
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
        "secure", "unsecure", "sendfile",
        "verify", "verified",
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

const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"];
const MAX_GIF_FRAMES: usize = 100;

/// Push a self-sent message to the chat log and spawn image download if the text
/// contains an image URL. Sender sees their own images inline.
fn push_self_message(
    app: &mut App,
    target: &str,
    text: String,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) {
    let nick = app.current_nickname.clone().unwrap_or_else(|| "?".to_string());
    let mut line = MessageLine { source: nick, text, kind: MessageKind::Privmsg, image_id: None };
    if app.render_images {
        if let Some(url) = extract_image_url(&line.text) {
            line.image_id = Some(app.next_image_id);
            app.next_image_id += 1;
            spawn_image_download(url, line.image_id.unwrap(), irc_tx, rt);
        }
    }
    app.push_message(target, line);
}

/// Spawn a background task to download an image URL, decode it, and send the
/// result back via the IRC message channel. Detects animated GIFs and extracts
/// all frames + delays.
fn spawn_image_download(
    url: &str,
    image_id: usize,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) {
    let url = url.to_string();
    let tx = irc_tx.clone();
    rt.spawn(async move {
        let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let resp = ureq::get(&url).call().map_err(|e| format!("{}", e))?;
            let len = resp
                .header("content-length")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            if len > 10_000_000 {
                return Err("Image too large (>10MB)".to_string());
            }
            let mut bytes = Vec::new();
            use std::io::Read as _;
            resp.into_reader()
                .take(10_000_000)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("{}", e))?;

            let is_gif = url.to_lowercase().ends_with(".gif");

            if is_gif {
                use image::codecs::gif::GifDecoder;
                use image::AnimationDecoder;
                if let Ok(decoder) = GifDecoder::new(std::io::Cursor::new(&bytes)) {
                    if let Ok(raw_frames) = decoder.into_frames().collect_frames() {
                        if raw_frames.len() > 1 {
                            let mut frames = Vec::with_capacity(raw_frames.len().min(MAX_GIF_FRAMES));
                            let mut delays = Vec::with_capacity(frames.capacity());
                            for frame in raw_frames.into_iter().take(MAX_GIF_FRAMES) {
                                let mut d = std::time::Duration::from(frame.delay());
                                if d < std::time::Duration::from_millis(20) {
                                    d = std::time::Duration::from_millis(20);
                                }
                                delays.push(d);
                                let img = image::DynamicImage::ImageRgba8(frame.into_buffer());
                                frames.push(img);
                            }
                            let _ = tx.send(IrcMessage::AnimatedImageReady {
                                image_id,
                                frames,
                                delays,
                            });
                            return Ok(());
                        }
                    }
                }
            }

            let img = image::load_from_memory(&bytes).map_err(|e| format!("{}", e))?;
            let _ = tx.send(IrcMessage::ImageReady { image_id, image: img });
            Ok(())
        })
        .await
        .map_err(|e| format!("{}", e))
        .and_then(|r| r);
        if let Err(_e) = result { }
    });
}

/// Sanitize a filename from a remote peer to prevent path traversal. Returns only the
/// last path component; if that is "." or ".." or empty, returns "file".
fn sanitize_received_filename(name: &str) -> String {
    let s = name.trim();
    if s.is_empty() {
        return "file".to_string();
    }
    let base = std::path::Path::new(s)
        .components()
        .last()
        .and_then(|c| c.as_os_str().to_str());
    match base {
        Some("") | Some(".") | Some("..") => "file".to_string(),
        Some(b) => b.to_string(),
        None => "file".to_string(),
    }
}

/// Return true if the string is a safe http(s) URL for image fetch (no newlines, nulls, or control chars).
fn is_safe_image_url(s: &str) -> bool {
    if !s.starts_with("http://") && !s.starts_with("https://") {
        return false;
    }
    !s.contains(|c: char| c == '\0' || c == '\n' || c == '\r' || c.is_control())
}

fn extract_image_url(text: &str) -> Option<&str> {
    for word in text.split_whitespace() {
        if (word.starts_with("http://") || word.starts_with("https://"))
            && IMAGE_EXTS
                .iter()
                .any(|ext| word.to_lowercase().ends_with(ext))
            && is_safe_image_url(word)
        {
            return Some(word);
        }
    }
    None
}

/// Parse a [:rvIRC:...] protocol message into a ProtocolEvent.
fn parse_rvirc_protocol(from_nick: &str, text: &str) -> Option<app::ProtocolEvent> {
    use app::ProtocolEvent;
    let inner = text.strip_prefix("[:rvIRC:")?;
    let inner = inner.strip_suffix(']').unwrap_or(inner);

    if let Some(rest) = inner.strip_prefix("SECURE:INIT:") {
        let mut parts = rest.splitn(2, ':');
        let ephemeral = parts.next()?;
        let identity = parts.next().unwrap_or("");
        return Some(ProtocolEvent::SecureInit {
            from_nick: from_nick.to_string(),
            ephemeral_pub_b64: ephemeral.to_string(),
            identity_pub_b64: identity.to_string(),
        });
    }
    if let Some(rest) = inner.strip_prefix("SECURE:ACK:") {
        let mut parts = rest.splitn(2, ':');
        let ephemeral = parts.next()?;
        let identity = parts.next().unwrap_or("");
        return Some(ProtocolEvent::SecureAck {
            from_nick: from_nick.to_string(),
            ephemeral_pub_b64: ephemeral.to_string(),
            identity_pub_b64: identity.to_string(),
        });
    }
    if let Some(rest) = inner.strip_prefix("ENC:") {
        let mut parts = rest.splitn(2, ':');
        let nonce = parts.next()?;
        let ct = parts.next()?;
        return Some(ProtocolEvent::Encrypted {
            from_nick: from_nick.to_string(),
            nonce_b64: nonce.to_string(),
            ciphertext_b64: ct.to_string(),
        });
    }
    if let Some(rest) = inner.strip_prefix("WORMHOLE:OFFER:") {
        let mut parts = rest.rsplitn(2, ':');
        let size_str = parts.next()?;
        let code_and_name = parts.next()?;
        let size = size_str.parse().unwrap_or(0);
        let mut code_filename = code_and_name.splitn(2, ':');
        let code = code_filename.next()?.to_string();
        let filename = code_filename.next().unwrap_or("file").to_string();
        return Some(ProtocolEvent::WormholeOffer {
            from_nick: from_nick.to_string(),
            code,
            filename: sanitize_received_filename(&filename),
            size,
        });
    }
    if inner.starts_with("WORMHOLE:COMPLETE") {
        return Some(ProtocolEvent::WormholeComplete {
            from_nick: from_nick.to_string(),
        });
    }
    if inner.starts_with("WORMHOLE:REJECT") {
        return Some(ProtocolEvent::WormholeReject {
            from_nick: from_nick.to_string(),
        });
    }
    None
}

/// Process queued protocol events (called from main loop, has access to client).
fn process_protocol_events(
    app: &mut App,
    client: &mut Option<Client>,
    rt: &tokio::runtime::Runtime,
    irc_tx: &IrcMessageTx,
) {
    use app::ProtocolEvent;
    let events: Vec<ProtocolEvent> = app.protocol_events.drain(..).collect();
    for evt in events {
        match evt {
            ProtocolEvent::SecureInit { from_nick, ephemeral_pub_b64, identity_pub_b64 } => {
                if !app.dm_targets.contains(&from_nick) {
                    app.dm_targets.push(from_nick.clone());
                }
                app.push_chat_log(&from_nick, &format!("*** {} REQUESTED SECURE CONNECTION ***", from_nick));

                // TOFU check
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                let tofu = app.known_keys.check(&from_nick, &server, &identity_pub_b64);
                let key_changed = matches!(tofu, TofuResult::KeyChanged);

                match &tofu {
                    TofuResult::FirstContact => {
                        let fp = key_fingerprint(&identity_pub_b64);
                        app.push_chat_log(&from_nick, &format!("First contact with {} -- key fingerprint: {}", from_nick, fp));
                    }
                    TofuResult::KeyMatch { verified } => {
                        if *verified {
                            app.push_chat_log(&from_nick, &format!("Key matches known VERIFIED identity for {}.", from_nick));
                        } else {
                            app.push_chat_log(&from_nick, &format!("Key matches known identity for {}.", from_nick));
                        }
                    }
                    TofuResult::KeyChanged => {
                        app.push_chat_log(&from_nick, &format!("⚠ WARNING: {}'s identity key has CHANGED since last session!", from_nick));
                        app.push_chat_log(&from_nick, "This could indicate a man-in-the-middle attack.");
                    }
                }

                // Show accept/reject popup
                app.secure_accept_popup_visible = true;
                app.secure_accept_nick = from_nick;
                app.secure_accept_ephemeral_b64 = ephemeral_pub_b64;
                app.secure_accept_identity_b64 = identity_pub_b64;
                app.secure_accept_key_changed = key_changed;
            }
            ProtocolEvent::SecureAck { from_nick, ephemeral_pub_b64, identity_pub_b64 } => {
                if app.pending_secure.remove(&from_nick) {
                    app.push_chat_log(&from_nick, "Key exchange response received...");

                    let their_identity_bytes: [u8; 32] = match base64::engine::general_purpose::STANDARD
                        .decode(&identity_pub_b64)
                        .ok()
                        .and_then(|v| v.try_into().ok())
                    {
                        Some(b) => b,
                        None => {
                            app.push_chat_log(&from_nick, "Secure handshake failed: invalid identity key.");
                            continue;
                        }
                    };

                    // TOFU check
                    let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                    let tofu = app.known_keys.check(&from_nick, &server, &identity_pub_b64);
                    match &tofu {
                        TofuResult::FirstContact => {
                            let fp = key_fingerprint(&identity_pub_b64);
                            app.push_chat_log(&from_nick, &format!("First contact with {} -- key fingerprint: {}", from_nick, fp));
                        }
                        TofuResult::KeyMatch { verified } => {
                            if *verified {
                                app.push_chat_log(&from_nick, &format!("Key matches known VERIFIED identity for {}.", from_nick));
                            } else {
                                app.push_chat_log(&from_nick, &format!("Key matches known identity for {}.", from_nick));
                            }
                        }
                        TofuResult::KeyChanged => {
                            app.push_chat_log(&from_nick, &format!("⚠ WARNING: {}'s identity key has CHANGED since last session!", from_nick));
                            app.push_chat_log(&from_nick, "This could indicate a man-in-the-middle attack. Session aborted.");
                            app.pending_secure_ephemeral.remove(&from_nick);
                            continue;
                        }
                    }

                    app.known_keys.upsert(&from_nick, &server, &identity_pub_b64);
                    if let Some(ref path) = app.known_keys_path {
                        let _ = app.known_keys.save(path);
                    }

                    let ephemeral = match app.pending_secure_ephemeral.remove(&from_nick) {
                        Some(kp) => kp,
                        None => {
                            app.push_chat_log(&from_nick, "Secure handshake failed: no ephemeral key found.");
                            continue;
                        }
                    };

                    match SecureSession::from_exchange(
                        &ephemeral.secret,
                        &ephemeral.public,
                        &ephemeral_pub_b64,
                        &app.keypair.public,
                        their_identity_bytes,
                    ) {
                        Ok(session) => {
                            app.secure_sessions.insert(from_nick.clone(), session);
                            let fp = key_fingerprint(&identity_pub_b64);
                            app.push_chat_log(&from_nick, &format!("Key fingerprint: {}", fp));
                            app.push_chat_log(&from_nick, "*** SECURE CONNECTION ESTABLISHED ***");
                            app.push_chat_log(&from_nick, "Messages are now end-to-end encrypted (X25519 + ChaCha20-Poly1305).");
                            if !app.known_keys.is_verified(&from_nick, &server) {
                                app.push_chat_log(&from_nick, "Use :verify to compare verification codes.");
                            }
                            app.status_message = format!("Secure session established with {}.", from_nick);
                        }
                        Err(e) => {
                            app.push_chat_log(&from_nick, &format!("Secure handshake failed: {}", e));
                            app.status_message = format!("Secure handshake with {} failed: {}", from_nick, e);
                        }
                    }
                }
            }
            ProtocolEvent::Encrypted { from_nick, nonce_b64, ciphertext_b64 } => {
                if let Some(session) = app.secure_sessions.get_mut(&from_nick) {
                    match session.decrypt(&nonce_b64, &ciphertext_b64) {
                        Ok(plaintext) => {
                            let mut line = MessageLine {
                                source: from_nick.clone(),
                                text: plaintext,
                                kind: MessageKind::Privmsg,
                                image_id: None,
                            };
                            if app.render_images {
                                if let Some(url) = extract_image_url(&line.text) {
                                    let image_id = app.next_image_id;
                                    app.next_image_id += 1;
                                    line.image_id = Some(image_id);
                                    spawn_image_download(url, image_id, irc_tx, rt);
                                }
                            }
                            app.push_message(&from_nick, line);
                        }
                        Err(e) => {
                            app.push_message(
                                &from_nick,
                                MessageLine {
                                    source: "rvIRC".to_string(),
                                    text: format!("[decrypt error: {}]", e),
                                    kind: MessageKind::Other,
                                    image_id: None,
                                },
                            );
                        }
                    }
                } else {
                    // No session: if we know this peer (TOFU), auto-initiate re-secure once
                    let server = app.current_server.as_deref().unwrap_or("unknown");
                    let known = app.known_keys.lookup(&from_nick, server).is_some();
                    let already_pending = app.pending_secure.contains(&from_nick);

                    if known && !already_pending {
                        if let Some(ref c) = client {
                            let ephemeral = crypto::Keypair::generate();
                            let ephemeral_pub_b64 = ephemeral.public_key_b64();
                            let identity_pub_b64 = app.keypair.public_key_b64();
                            let msg = format!("[:rvIRC:SECURE:INIT:{}:{}]", ephemeral_pub_b64, identity_pub_b64);
                            if c.send_privmsg(&from_nick, &msg).is_ok() {
                                app.pending_secure.insert(from_nick.clone());
                                app.pending_secure_ephemeral.insert(from_nick.clone(), ephemeral);
                                if !app.dm_targets.contains(&from_nick) {
                                    app.dm_targets.push(from_nick.clone());
                                }
                                app.push_chat_log(&from_nick, "*** No secure session. Sending key exchange to re-establish... ***");
                            }
                        }
                    }

                    app.push_message(
                        &from_nick,
                        MessageLine {
                            source: from_nick.clone(),
                            text: "[encrypted message, no secure session]".to_string(),
                            kind: MessageKind::Other,
                            image_id: None,
                        },
                    );
                }
            }
            ProtocolEvent::WormholeOffer { from_nick, code, filename, size } => {
                let size_display = if size >= 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size >= 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} B", size)
                };
                app.push_chat_log(&from_nick, &format!(
                    "*** {} wants to send you {} ({}) ***",
                    from_nick, filename, size_display
                ));
                app.file_receive_popup_visible = true;
                app.file_receive_nick = from_nick;
                app.file_receive_filename = filename; // already sanitized in parse_rvirc_protocol
                app.file_receive_size = size;
                app.file_receive_code = code;
            }
            ProtocolEvent::WormholeComplete { from_nick } => {
                app.push_chat_log(&from_nick, "*** File transfer completed. ***");
                app.status_message = format!("File transfer to {} completed.", from_nick);
            }
            ProtocolEvent::WormholeReject { from_nick } => {
                app.push_chat_log(&from_nick, "*** File transfer rejected. ***");
                app.status_message = format!("{} rejected the file transfer.", from_nick);
            }
        }
    }
}
