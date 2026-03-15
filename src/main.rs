//! rvIRC - Rust + VIM + IRC. Terminal IRC client with vim-style commands.

mod app;
mod commands;
mod config;
mod connection;
mod sts;
mod crypto;
mod events;
mod filetransfer;
mod format;
mod friends;
mod highlight;
mod read_markers;
mod notifications;
mod ui;

use app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use config::RvConfig;
use connection::{connect, run_stream, IrcMessage, IrcMessageTx};
use crypto::{KnownKeys, SecureSession, TofuResult, key_fingerprint};
use base64::Engine;
use events::{handle_key, KeyAction};
use irc::client::prelude::*;
use irc::proto::message::Tag;
use irc::proto::Command as IrcCommand;
use irc::proto::{ChannelMode as IrcChannelMode, Message as IrcProtoMessage, Mode as IrcMode};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use tokio::sync::mpsc;

fn main() -> Result<(), String> {
    let config = RvConfig::load()?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;

    let (irc_tx, mut irc_rx) = mpsc::unbounded_channel::<IrcMessage>();

    let mut app = App::new();
    app.render_images = config.render_images;
    app.offline_friends = config.offline_friends.clone();
    app.notifications_enabled = config.notifications;
    app.sounds_enabled = config.sounds;
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
        let friends_path = config_dir.join("friends.toml");
        app.friends_path = Some(friends_path);
        let highlight_path = config_dir.join("highlight.toml");
        app.highlight_path = Some(highlight_path.clone());
        app.highlight_words = crate::highlight::load_highlights(&highlight_path);
        app.read_markers_path = Some(config_dir.join("read_markers.toml"));
    }

    let sts_path = RvConfig::config_dir().map(|d| d.join("sts.toml")).unwrap_or_default();
    let mut sts_policies = sts::StsPolicies::load(&sts_path);

    let mut clients: HashMap<String, (Client, tokio::task::JoinHandle<()>)> = HashMap::new();

    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());

    let mut terminal = setup_terminal().map_err(|e| e.to_string())?;
    let mut auto_connect_attempted = false;

    loop {
        terminal.draw(|f| ui::draw(f, &mut app)).map_err(|e| e.to_string())?;

        // Auto-connect once on startup to all servers with auto_connect = "yes"
        if !auto_connect_attempted && clients.is_empty() {
            auto_connect_attempted = true;
            let auto_connect_servers: Vec<_> = config.servers.iter().filter(|s| s.is_auto_connect()).collect();
            for server in auto_connect_servers {
                let initial_away = app.away_message.clone();
                match connect(server, &config, irc_tx.clone(), &rt, initial_away, &sts_policies) {
                    Ok((c, stream)) => {
                        let name = server.name.clone();
                        let name_for_spawn = name.clone();
                        let host = server.host.clone();
                        let use_tls = sts_policies.get_valid(&server.host).is_some() || server.tls;
                        let tx = irc_tx.clone();
                        let our_nick = config.nickname.clone();
                        let handle = rt.spawn(async move {
                            run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await;
                        });
                        clients.insert(name.clone(), (c, handle));
                        if app.current_server.is_none() {
                            app.current_server = Some(name.clone());
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read(&name, "*server*");
                            app.channel_index = 0;
                        }
                        app.current_nickname = config.nickname.clone();
                        app.status_message = format!("Auto-connecting to {}...", name);
                        if server.sasl_mechanism.is_none() {
                            if let Some(ref pw) = server.identify_password {
                                if let Some((ref c, _)) = clients.get(name.as_str()) {
                                    let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                }
                                app.auto_join_after_per_server.insert(name.clone(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                            }
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
                M::CtcpRequest { server, from_nick, tag, data, .. } => {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
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
                M::NickInUse { server } => {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        if let Some(ref alt) = config.alt_nick {
                            let _ = c.send(IrcCommand::NICK(alt.clone()));
                            app.status_message = format!("Nick in use, trying {}...", alt);
                        } else {
                            app.status_message = "Nickname in use.".to_string();
                        }
                    }
                }
                M::SendPrivmsg { server, target, text } => {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        for chunk in format::split_message_for_irc(text, format::MAX_MESSAGE_BYTES) {
                            let _ = c.send_privmsg(target, &chunk);
                        }
                    }
                }
                M::SendPrivmsgOrEncrypt { server, target, text } => {
                    let sec_key = app::msg_key(server, target);
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        if let Some(session) = app.secure_sessions.get_mut(&sec_key) {
                            for chunk in format::split_message_for_irc(text, format::MAX_ENCRYPTED_PLAINTEXT_BYTES) {
                                match session.encrypt(&chunk) {
                                    Ok((nonce, ct)) => {
                                        let wire = format!("[:rvIRC:ENC:{}:{}]", nonce, ct);
                                        let _ = c.send_privmsg(target, &wire);
                                    }
                                    Err(_) => {
                                        let _ = c.send_privmsg(target, &chunk);
                                    }
                                }
                            }
                        } else {
                            for chunk in format::split_message_for_irc(text, format::MAX_MESSAGE_BYTES) {
                                let _ = c.send_privmsg(target, &chunk);
                            }
                        }
                    }
                }
                M::Status(ref s) => {
                    app.status_message = s.clone();
                }
                M::ChatLog { server, target, text } => {
                    app.push_message(
                        server,
                        target,
                        MessageLine {
                            source: "***".to_string(),
                            text: text.clone(),
                            kind: MessageKind::Other,
                            image_id: None,
                            timestamp: None,
                            account: None,
                            msgid: None,
                            reply_to_msgid: None,
                            is_bot_sender: false,
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
                M::TransferProgress { server: _, nick, filename, bytes, total, is_send } => {
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
                M::RequestChathistory { server, target } => {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let use_labeled = app.acked_caps_per_server.get(server).map_or(false, |s| s.contains("labeled-response"));
                        if use_labeled {
                            let label = format!("{:016x}", rand::random::<u64>());
                            let tags = Some(vec![Tag("label".to_string(), Some(label))]);
                            let args: Vec<&str> = vec!["LATEST", target.as_str(), "*", "50"];
                            if let Ok(msg) = IrcProtoMessage::with_tags(tags, None, "CHATHISTORY", args) {
                                let _ = c.send(msg);
                            } else {
                                let _ = c.send(IrcCommand::Raw("CHATHISTORY".to_string(), vec!["LATEST".to_string(), target.clone(), "*".to_string(), "50".to_string()]));
                            }
                        } else {
                            let _ = c.send(IrcCommand::Raw(
                                "CHATHISTORY".to_string(),
                                vec!["LATEST".to_string(), target.clone(), "*".to_string(), "50".to_string()],
                            ));
                        }
                    }
                }
                M::RequestChathistoryBefore { server, target, reference } => {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let use_labeled = app.acked_caps_per_server.get(server).map_or(false, |s| s.contains("labeled-response"));
                        if use_labeled {
                            let label = format!("{:016x}", rand::random::<u64>());
                            let tags = Some(vec![Tag("label".to_string(), Some(label))]);
                            let args: Vec<&str> = vec!["BEFORE", target.as_str(), reference.as_str(), "50"];
                            if let Ok(msg) = IrcProtoMessage::with_tags(tags, None, "CHATHISTORY", args) {
                                let _ = c.send(msg);
                            } else {
                                let _ = c.send(IrcCommand::Raw("CHATHISTORY".to_string(), vec!["BEFORE".to_string(), target.clone(), reference.clone(), "50".to_string()]));
                            }
                        } else {
                            let _ = c.send(IrcCommand::Raw(
                                "CHATHISTORY".to_string(),
                                vec!["BEFORE".to_string(), target.clone(), reference.clone(), "50".to_string()],
                            ));
                        }
                    }
                }
                M::ChathistoryBatch { server, target, lines } => {
                    app.chathistory_before_pending = None;
                    let key = app::msg_key(server, target);
                    let buf = app.messages.entry(key).or_default();
                    for line in lines.iter().rev() {
                        if line.is_bot_sender && !line.source.is_empty() {
                            app.bot_per_nick.insert((server.clone(), line.source.to_lowercase()));
                        }
                        buf.insert(0, line.clone());
                    }
                }
                M::StsPolicyReceived { host, port, duration_secs } => {
                    sts_policies.set(host, *port, *duration_secs);
                    let _ = sts_policies.save(&sts_path);
                }
                M::StsUpgradeRequired { server, host: _host, port: sts_port } => {
                    if let Some((_c, handle)) = clients.remove(server.as_str()) {
                        handle.abort();
                    }
                    if let Some(entry) = config.server_by_name(server) {
                        let mut override_entry = entry.clone();
                        override_entry.port = *sts_port;
                        override_entry.tls = true;
                        app.status_message = format!("Upgrading to TLS on port {}...", sts_port);
                        let initial_away = app.away_message.clone();
                        match connect(&override_entry, &config, irc_tx.clone(), &rt, initial_away, &sts_policies) {
                            Ok((c, stream)) => {
                                let name = server.clone();
                                let name_for_spawn = name.clone();
                                let host = override_entry.host.clone();
                                let use_tls = true;
                                let tx = irc_tx.clone();
                                let our_nick = config.nickname.clone();
                                let handle = rt.spawn(async move {
                                    run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await;
                                });
                                clients.insert(name.clone(), (c, handle));
                                if app.current_server.is_none() {
                                    app.current_server = Some(name.clone());
                                    app.current_channel = Some("*server*".to_string());
                                    app.mark_target_read(&name, "*server*");
                                    app.channel_index = 0;
                                }
                                app.current_nickname = config.nickname.clone();
                                app.status_message = "Upgraded to TLS (STS).".to_string();
                                app.pending_auto_join_servers.insert(name.clone());
                                if override_entry.sasl_mechanism.is_none() {
                                    if let Some(ref pw) = override_entry.identify_password {
                                        if let Some((ref c, _)) = clients.get(name.as_str()) {
                                            let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                            app.status_message = "Identifying with NickServ...".to_string();
                                        }
                                        app.auto_join_after_per_server.insert(name, std::time::Instant::now() + std::time::Duration::from_secs(2));
                                    }
                                }
                            }
                            Err(e) => {
                                app.status_message = format!("STS upgrade failed: {}. Reconnect manually.", e);
                                app.reconnect_server = Some(server.clone());
                                app.reconnect_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
                                app.reconnect_attempt = 1;
                            }
                        }
                    }
                }
                _ => {}
            }
            let _was_connected = matches!(&msg, M::Connected { .. });
            let connected_server = if let M::Connected { ref server } = &msg {
                Some(server.clone())
            } else {
                None
            };
            apply_irc_message(&mut app, msg, &irc_tx, &rt);
            if let Some(server) = connected_server {
                if let Some((ref c, _)) = clients.get(server.as_str()) {
                    for nick in &app.friends_list {
                        let _ = c.send(IrcCommand::MONITOR("+".to_string(), Some(nick.clone())));
                    }
                }
            }
        }

        if !app.protocol_events.is_empty() {
            process_protocol_events(&mut app, &clients, &rt, &irc_tx);
        }

        // Auto-join channels after connect: per-server, identify first then join (delay when we identified)
        let mut joined_any = false;
        let pending: Vec<_> = app.pending_auto_join_servers.iter().cloned().collect();
        for server_name in pending {
            let can_join = app.auto_join_after_per_server
                .get(&server_name)
                .map_or(true, |t| std::time::Instant::now() >= *t);
            if can_join {
                app.pending_auto_join_servers.remove(&server_name);
                app.auto_join_after_per_server.remove(&server_name);
                if let (Some((ref c, _)), Some(server)) = (clients.get(server_name.as_str()), config.server_by_name(&server_name)) {
                    let channels = server.auto_join_channels();
                    for ch in &channels {
                        let _ = c.send_join(ch);
                        let _ = c.send_topic(ch, "");
                        let chans = app.channels_per_server.entry(server_name.clone()).or_default();
                        if !chans.contains(ch) {
                            chans.push(ch.clone());
                        }
                    }
                    if let Some(first) = channels.first() {
                        if app.current_server.as_deref() == Some(server_name.as_str()) {
                            app.current_channel = Some(first.clone());
                            app.mark_target_read(&server_name, first);
                            app.sync_channel_index_to_current();
                        }
                    }
                    if !channels.is_empty() {
                        joined_any = true;
                        app.status_message = format!("Joined {} channel(s) on {}.", channels.len(), server_name);
                    }
                }
            }
        }
        if joined_any {
            app.clamp_channel_index();
        }

        // Auto-reconnect: 3 attempts at 5s, 15s, 30s after disconnect
        if app.reconnect_after.is_some()
            && std::time::Instant::now() >= app.reconnect_after.unwrap()
        {
            let server_name = app.reconnect_server.clone();
            app.reconnect_after = None;
            if let Some(server_name) = server_name {
                if let Some(server) = config.server_by_name(&server_name) {
                    app.status_message = format!("Reconnecting to {} (attempt {})...", server_name, app.reconnect_attempt);
                    let initial_away = app.away_message.clone();
                    match connect(server, &config, irc_tx.clone(), &rt, initial_away, &sts_policies) {
                        Ok((c, stream)) => {
                            let name = server_name.clone();
                            let name_for_spawn = name.clone();
                            let host = server.host.clone();
                            let use_tls = sts_policies.get_valid(&server.host).is_some() || server.tls;
                            let tx = irc_tx.clone();
                            let our_nick = config.nickname.clone();
                            let handle = rt.spawn(async move { run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await });
                            clients.insert(name.clone(), (c, handle));
                            app.current_server = Some(name.clone());
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read(&name, "*server*");
                            app.channel_index = 0;
                            app.clear_reconnect();
                            if server.sasl_mechanism.is_none() {
                                if let Some(ref pw) = server.identify_password {
                                    if let Some((ref c, _)) = clients.get(name.as_str()) {
                                        let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                        app.status_message = "Identifying with NickServ...".to_string();
                                    }
                                    app.auto_join_after_per_server.insert(name.clone(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                                }
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
                .and_then(|ev| handle_key(ev, app.mode, app.panel_focus, app.reply_select_mode, app.channel_panel_visible, app.messages_panel_visible, app.user_panel_visible, app.friends_panel_visible, app.user_action_menu, app.channel_list_popup_visible, app.channel_list_scroll_mode, app.search_popup_visible, app.search_scroll_mode, app.server_list_popup_visible, app.whois_popup_visible, app.credits_popup_visible, app.license_popup_visible, app.file_receive_popup_visible, app.file_browser_visible, app.secure_accept_popup_visible, app.highlight_popup_visible, app.away_popup_visible, app.user_list_filter_focused))
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
                &mut clients,
                &irc_tx,
                &rt,
                &sts_policies,
                &sts_path,
                action,
            )?;
            if quit {
                break;
            }
        }
    }

    // Clean disconnect so the server and other users see a proper QUIT (not just connection closed)
    for (_, (c, h)) in std::mem::take(&mut clients) {
        let _ = c.send_quit("Leaving");
        h.abort();
    }
    std::thread::sleep(std::time::Duration::from_millis(250));

    restore_terminal().map_err(|e| e.to_string())?;
    Ok(())
}

/// Send IRCv3 typing indicator (TAGMSG with +typing=active|done). Throttles active to once per 3s.
fn send_typing_indicator(app: &mut App, clients: &HashMap<String, (Client, tokio::task::JoinHandle<()>)>, status: &str) {
    let target = match app.current_channel.as_deref() {
        Some(t) if t != "*server*" => t,
        _ => {
            if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                app.status_message = "typing skip: no valid target".to_string();
            }
            return;
        }
    };
    let c = match app.current_server.as_ref().and_then(|s| clients.get(s)) {
        Some((c, _)) => c,
        None => {
            if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                app.status_message = "typing skip: not connected".to_string();
            }
            return;
        }
    };
    let server = app.current_server.as_deref().unwrap_or("");
    let key = app::msg_key(server, target);
    if status == "active" {
        let now = std::time::Instant::now();
        if let Some(&last) = app.last_typing_sent.get(&key) {
            if now.duration_since(last).as_secs() < 3 {
                return; // throttle: 3s between active, no status spam
            }
        }
        app.last_typing_sent.insert(key.clone(), now);
    } else if status == "done" {
        app.last_typing_sent.remove(&key);
    }
    let tags = Some(vec![Tag("+typing".to_string(), Some(status.to_string()))]);
    match IrcProtoMessage::with_tags(tags, None, "TAGMSG", vec![target]) {
        Ok(msg) => {
            if let Err(e) = c.send(msg) {
                app.status_message = format!("typing send error: {}", e);
            } else if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                app.status_message = format!("typing sent: {} -> {}", target, status);
            }
        }
        Err(e) => {
            app.status_message = format!("typing build error: {}", e);
        }
    }
}

fn apply_irc_message(
    app: &mut App,
    msg: IrcMessage,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) {
    use app::msg_key;
    use connection::IrcMessage as M;
    match msg {
        M::Line { server, target, mut line } => {
            if line.is_bot_sender && !line.source.is_empty() {
                app.bot_per_nick.insert((server.clone(), line.source.to_lowercase()));
            }
            if line.text.starts_with("[:rvIRC:") {
                if let Some(evt) = parse_rvirc_protocol(&line.source, &line.text) {
                    app.protocol_events.push(evt);
                }
                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                if !dms.contains(&line.source) {
                    dms.push(line.source.clone());
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
            // Skip echoed self-messages when we have echo-message: we already showed our message via push_self_message with correct reply_to.
            let is_echo_from_us = app.current_nickname.as_ref().map_or(false, |n| line.source.eq_ignore_ascii_case(n));
            let has_echo = app.acked_caps_per_server.get(&server).map_or(false, |s| s.contains("echo-message"));
            if is_echo_from_us && has_echo {
                return;
            }
            let (effective_target, source, text) = if target == app.current_nickname.as_deref().unwrap_or("") {
                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                if !dms.contains(&line.source) {
                    dms.push(line.source.clone());
                }
                app.push_message(&server, &line.source, line.clone());
                (line.source.clone(), line.source.clone(), line.text.clone())
            } else {
                app.push_message(&server, &target, line.clone());
                (target.clone(), line.source.clone(), line.text.clone())
            };
            app.typing_status.remove(&(server.clone(), source.clone(), effective_target.clone()));
            if let Some(ref our_nick) = app.current_nickname {
                app.typing_status.remove(&(server.clone(), source.clone(), our_nick.clone()));
            }
            if line.kind == MessageKind::Quit {
                app.typing_status.retain(|(_, n, _), _| n != &source);
            } else if line.kind == MessageKind::Part {
                app.typing_status.remove(&(server.clone(), source.clone(), effective_target.clone()));
            }
            let current_key = app.current_server.as_ref().and_then(|s| {
                app.current_channel.as_ref().map(|t| msg_key(s, t))
            }).unwrap_or_default();
            let key = msg_key(&server, &effective_target);
            if key != current_key && !app.is_muted(&key, &source) {
                let preview = format!("{}: {}", source, text);
                let preview = preview.chars().take(80).collect::<String>();
                if app.notifications_enabled {
                    notifications::show_desktop(&effective_target, &preview);
                }
                if app.sounds_enabled {
                    notifications::play_bell();
                }
            }
        }
        M::JoinedChannel { server, channel } => {
            let chans = app.channels_per_server.entry(server).or_default();
            if !chans.contains(&channel) {
                chans.push(channel);
            }
        }
        M::PartedChannel { server, channel } => {
            if let Some(chans) = app.channels_per_server.get_mut(&server) {
                chans.retain(|c| c != &channel);
            }
            app.clamp_channel_index();
            if app.current_server.as_deref() == Some(server.as_str()) && app.current_channel.as_deref() == Some(channel.as_str()) {
                app.save_current_read_marker();
                if let Some((s, t)) = app.selected_channel_entry().or_else(|| app.selected_message_entry()) {
                    app.current_server = Some(s.clone());
                    app.current_channel = Some(t.clone());
                    app.restore_read_marker_for(&s, &t);
                    app.mark_target_read(&s, &t);
                }
            }
        }
        M::UserList { server, channel, users, userhosts } => {
            if app.current_server.as_deref() == Some(server.as_str()) && app.current_channel.as_deref() == Some(channel.as_str()) {
                app.set_user_list(&server, users, userhosts);
            }
        }
        M::ChannelList { server, list } => {
            let with_server: Vec<_> = list
                .into_iter()
                .map(|(ch, count)| (server.clone(), ch, count))
                .collect();
            if app.channel_list_super {
                app.channel_list_pending_servers.remove(&server);
                app.server_channel_list.extend(with_server);
                app.server_channel_list
                    .sort_by(|a, b| b.2.unwrap_or(0).cmp(&a.2.unwrap_or(0)));
                app.clamp_channel_list_selected_index();
                if app.channel_list_pending_servers.is_empty() {
                    app.status_message = format!("{} channels (all servers)", app.server_channel_list.len());
                }
            } else {
                app.server_channel_list = with_server;
                app.server_channel_list
                    .sort_by(|a, b| b.2.unwrap_or(0).cmp(&a.2.unwrap_or(0)));
                app.clamp_channel_list_selected_index();
                app.status_message = format!("{} channels", app.server_channel_list.len());
            }
        }
        M::WhoisResult { server: _server, nick, lines } => {
            app.whois_nick = nick;
            app.whois_lines = lines;
            app.whois_popup_visible = true;
        }
        M::Topic { server, channel, topic } => {
            let key = msg_key(&server, &channel);
            app.channel_topics.insert(key, topic.unwrap_or_default());
        }
        M::ChannelModes { server, channel, modes } => {
            let key = msg_key(&server, &channel);
            app.channel_modes.insert(key, modes);
        }
        M::Invite { server: _server, inviter, target, channel } => {
            let our_nick = app.current_nickname.as_deref().unwrap_or("");
            if target.eq_ignore_ascii_case(our_nick) {
                app.last_invite = Some((inviter.clone(), channel.clone()));
                app.status_message = format!("{} invited you to {} (use :join {} to join)", inviter, channel, channel);
            } else {
                app.status_message = format!("{} invited {} to {}", inviter, target, channel);
            }
        }
        M::Typing { server, nick, target, status } => {
            if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                app.status_message = format!("typing stored: {} in {} = {}", nick, target, status);
            }
            if status == "done" {
                app.typing_status.remove(&(server, nick, target));
            } else {
                app.typing_status.insert((server, nick, target), (status, std::time::Instant::now()));
            }
        }
        M::CapRequested { server, caps } => {
            app.requested_caps_per_server.insert(server, caps);
        }
        M::CapsAcked { server, caps } => {
            let set = app.acked_caps_per_server.entry(server).or_default();
            for c in caps {
                set.insert(c);
            }
            if std::env::var("RVIRC_DEBUG_TYPING").is_ok() {
                let has_mt = app.current_server.as_ref().and_then(|s| app.acked_caps_per_server.get(s))
                    .map(|m| m.contains("message-tags")).unwrap_or(false);
                app.status_message = format!("caps acked: message-tags={}", has_mt);
            }
        }
        M::CapsNak { .. } => {}
        M::CapsChanged { server, added, removed } => {
            let set = app.acked_caps_per_server.entry(server.clone()).or_default();
            for c in added {
                set.insert(c);
            }
            let sasl_dropped = removed.iter().any(|c| c.eq_ignore_ascii_case("sasl"));
            for c in removed {
                set.remove(&c);
            }
            if sasl_dropped {
                app.status_message = "Server capabilities changed (SASL may have been dropped); consider reconnecting.".to_string();
            }
        }
        M::NickInUse { .. } | M::CtcpRequest { .. } | M::SendPrivmsg { .. } | M::SendPrivmsgOrEncrypt { .. } | M::Status(_) | M::ChatLog { .. } | M::ImageReady { .. } | M::AnimatedImageReady { .. } | M::TransferProgress { .. } | M::TransferComplete { .. } => {}
        M::MonOnline { server: _server, nicks } => {
            for n in nicks {
                app.friends_online.insert(n);
            }
        }
        M::MonOffline { server: _server, nicks } => {
            for n in nicks {
                app.friends_online.remove(&n);
                app.friends_away.remove(&n);
            }
            app.clamp_friends_index();
        }
        M::FriendAway { server: _server, nick, away } => {
            if app.friends_list.iter().any(|n| n.eq_ignore_ascii_case(&nick)) {
                if let Some(existing) = app.friends_away.iter().find(|a| a.eq_ignore_ascii_case(&nick)).cloned() {
                    app.friends_away.remove(&existing);
                }
                if away {
                    app.friends_away.insert(nick);
                }
            }
        }
        M::AccountUpdate { server, nick, account } => {
            let key = (server.clone(), nick.to_lowercase());
            app.account_per_nick.insert(key, account);
        }
        M::ChgHost { server: _server, nick, new_user, new_host } => {
            app.status_message = format!("{} changed host to {}@{}", nick, new_user, new_host);
        }
        M::Setname { server: _server, nick, realname } => {
            app.status_message = format!("{} is now known as {}", nick, realname);
        }
        M::Connected { server } => {
            if !app.connected_servers.contains(&server) {
                app.connected_servers.push(server.clone());
            }
            if app.current_server.is_none() {
                app.current_server = Some(server.clone());
            }
            app.friends_online.clear();
            app.friends_away.clear();
            if let Some(ref path) = app.friends_path {
                app.friends_list = friends::load_friends(path, Some(&server));
                app.friends_index = 0;
                app.clamp_friends_index();
            }
            app.status_message = "Connected.".to_string();
            app.pending_auto_join_servers.insert(server);
        }
        M::Disconnected { server } => {
            let server_for_reconnect = server.clone();
            app.connected_servers.retain(|s| s != &server);
            app.channels_per_server.remove(&server);
            app.dm_targets_per_server.remove(&server);
            app.typing_status.retain(|(s, _, _), _| s != &server);
            app.last_typing_sent.retain(|k, _| !k.starts_with(&format!("{}/", server)));
            for k in app.messages.keys().cloned().collect::<Vec<_>>() {
                if k.starts_with(&format!("{}/", server)) {
                    app.messages.remove(&k);
                }
            }
            // Clear reactions (msgids are orphaned when messages removed)
            app.reactions.clear();
            app.unread_targets.retain(|k| !k.starts_with(&format!("{}/", server)));
            app.unread_mentions.retain(|k| !k.starts_with(&format!("{}/", server)));
            for k in app.channel_topics.keys().cloned().collect::<Vec<_>>() {
                if k.starts_with(&format!("{}/", server)) {
                    app.channel_topics.remove(&k);
                }
            }
            for k in app.channel_modes.keys().cloned().collect::<Vec<_>>() {
                if k.starts_with(&format!("{}/", server)) {
                    app.channel_modes.remove(&k);
                }
            }
            if app.current_server.as_deref() == Some(server.as_str()) {
                app.current_server = app.connected_servers.first().cloned();
                app.current_channel = app.current_server.as_ref().map(|_| "*server*".to_string());
                app.sync_channel_index_to_current();
            }
            app.clamp_channel_index();
            app.clamp_messages_index();
            app.user_list.clear();
            app.search_popup_visible = false;
            app.channel_list_popup_visible = false;
            app.server_channel_list.clear();
            app.channel_list_filter.clear();
            app.channel_list_scroll_mode = false;
            app.channel_list_server = None;
            app.channel_list_super = false;
            app.channel_list_pending_servers.clear();
            app.server_list_popup_visible = false;
            app.highlight_popup_visible = false;
            app.whois_popup_visible = false;
            app.whois_nick.clear();
            app.whois_lines.clear();
            app.pending_auto_join_servers.remove(&server);
            app.auto_join_after_per_server.remove(&server);
            app.last_invite = None;
            app.friends_online.clear();
            app.friends_away.clear();
            app.acked_caps_per_server.remove(&server);
            app.requested_caps_per_server.remove(&server);
            app.away_message = None;
            app.away_popup_visible = false;
            app.status_message = "Disconnected.".to_string();
            app.reconnect_server = Some(server_for_reconnect);
            app.reconnect_after = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
            app.reconnect_attempt = 1;
        }
        M::MessageRedacted { server, target, msgid } => {
            let key = app::msg_key(&server, &target);
            if let Some(buf) = app.messages.get_mut(&key) {
                if let Some(m) = buf.iter_mut().find(|l| l.msgid.as_deref() == Some(msgid.as_str())) {
                    m.text = "[Message redacted]".to_string();
                    m.kind = MessageKind::Other;
                }
            }
            app.reactions.remove(&msgid);
        }
        M::Reaction { server: _server, target: _target, msgid, nick, emoji, unreact } => {
            let list = app.reactions.entry(msgid.clone()).or_default();
            if unreact {
                list.retain(|(n, e)| !(n == &nick && e == &emoji));
            } else {
                list.push((nick, emoji));
            }
        }
                M::StsPolicyReceived { .. } | M::StsUpgradeRequired { .. } | M::RequestChathistory { .. } | M::RequestChathistoryBefore { .. } | M::ChathistoryBatch { .. } => {
            // Handled in the outer match block before apply_irc_message
        }
        M::StandardReply { server, kind, command: _command, code: _code, description } => {
            use connection::StandardReplyKind;
            let (source, _) = match kind {
                StandardReplyKind::Fail => ("[FAIL]", ()),
                StandardReplyKind::Warn => ("[WARN]", ()),
                StandardReplyKind::Note => ("[NOTE]", ()),
            };
            let line = MessageLine {
                source: source.to_string(),
                text: description,
                kind: MessageKind::Other,
                image_id: None,
                timestamp: None,
                account: None,
                msgid: None,
                reply_to_msgid: None,
                is_bot_sender: false,
            };
            app.push_message(&server, "*server*", line);
        }
    }
}

/// Previous character boundary (for cursor left). Returns 0 if already at start.
fn input_prev_char_boundary(s: &str, i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    let mut j = i - 1;
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

/// Next character boundary (for cursor right). Returns s.len() if already at end.
fn input_next_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let next_char_len = s[i..].chars().next().map_or(0, |c| c.len_utf8());
    i + next_char_len
}

fn handle_key_action(
    app: &mut App,
    config: &RvConfig,
    clients: &mut HashMap<String, (Client, tokio::task::JoinHandle<()>)>,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
    sts_policies: &sts::StsPolicies,
    sts_path: &std::path::Path,
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
                app.input_selection = None;
            }
        }
        Reply => {
            // Enter reply-select mode: show numbers 1–9, 0 on last 10 messages; user picks one.
            let ids = app.replyable_msgids();
            if ids.is_empty() {
                app.status_message = "No message with reply support in this buffer.".to_string();
            } else {
                app.reply_select_mode = true;
                app.status_message = "Press 1–9 or 0 to pick message (Esc to cancel).".to_string();
            }
        }
        ReplySelectByNumber(n) => {
            let ids = app.replyable_msgids();
            let idx = if n == 10 { 9 } else { (n as usize).saturating_sub(1) };
            if idx < ids.len() {
                let msgid = ids[ids.len() - 1 - idx].clone();
                app.reply_to_msgid = Some(msgid);
                app.reply_select_mode = false;
                app.mode = Mode::Insert;
                app.status_message = "Replying (Esc to cancel).".to_string();
            }
        }
        ReplySelectCancel => {
            app.reply_select_mode = false;
        }
        FocusChannels => {
            if app.channel_panel_visible {
                app.panel_focus = PanelFocus::Channels;
            }
        }
        FocusMessages => {
            if app.messages_panel_visible {
                app.panel_focus = PanelFocus::Messages;
            }
        }
        FocusUsers => {
            if app.user_panel_visible {
                app.panel_focus = PanelFocus::Users;
                if app.current_channel.as_ref().map_or(false, |t| t.starts_with('#') || t.starts_with('&')) {
                    request_channel_names(clients, app);
                }
            }
        }
        FocusFriends => {
            if app.friends_panel_visible {
                app.panel_focus = PanelFocus::Friends;
            }
        }
        UnfocusPanel => {
            if app.panel_focus == PanelFocus::Users {
                app.user_list_filter_focused = false;
            }
            app.panel_focus = PanelFocus::Main;
        }
        FocusInputAndType(c) => {
            if app.panel_focus == PanelFocus::Users {
                app.user_list_filter_focused = false;
            }
            app.panel_focus = PanelFocus::Main;
            app.mode = Mode::Insert;
            if app.input.len() < format::MAX_INPUT_BYTES {
                app.input.insert(app.input_cursor, c);
                app.input_cursor += c.len_utf8();
            }
        }
        ChannelUp => {
            app.channel_index = app.channel_index.saturating_sub(1);
        }
        ChannelDown => {
            let len = app.channels_list().len();
            if app.channel_index + 1 < len {
                app.channel_index += 1;
            }
        }
        MessageUp => {
            app.messages_index = app.messages_index.saturating_sub(1);
        }
        MessageDown => {
            if app.messages_index + 1 < app.messages_list().len() {
                app.messages_index += 1;
            }
        }
        MessageSelect => {
            if let Some((server, nick)) = app.messages_list().get(app.messages_index).cloned() {
                app.save_current_read_marker();
                app.current_server = Some(server.clone());
                app.current_channel = Some(nick.clone());
                app.mark_target_read(&server, &nick);
                app.restore_read_marker_for(&server, &nick);
                app.panel_focus = PanelFocus::Main;
            }
        }
        MessageSelectByNumber(n) => {
            let list = app.messages_list();
            let idx = if n == 10 { 9 } else { (n as usize).saturating_sub(1) };
            let idx = idx.min(list.len().saturating_sub(1));
            if idx < list.len() {
                app.messages_index = idx;
                if let Some((server, nick)) = list.get(idx).cloned() {
                    app.save_current_read_marker();
                    app.current_server = Some(server.clone());
                    app.current_channel = Some(nick.clone());
                    app.mark_target_read(&server, &nick);
                    app.restore_read_marker_for(&server, &nick);
                    app.panel_focus = PanelFocus::Main;
                }
            }
        }
        FriendUp => {
            app.friends_index = app.friends_index.saturating_sub(1);
        }
        FriendDown => {
            if app.friends_index + 1 < app.visible_friends().len() {
                app.friends_index += 1;
            }
        }
        FriendSelect => {
            if let Some(nick) = app.selected_friend() {
                let server = app.current_server.as_ref().cloned().unwrap_or_default();
                if server.is_empty() {
                    return Ok(false);
                }
                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                if !dms.contains(&nick) {
                    dms.push(nick.clone());
                }
                app.save_current_read_marker();
                app.current_channel = Some(nick.clone());
                app.mark_target_read(&server, &nick);
                app.restore_read_marker_for(&server, &nick);
                app.sync_channel_index_to_current();
                app.panel_focus = PanelFocus::Main;
            }
        }
        ChannelSelect => {
            if let Some((server, target)) = app.channels_list().get(app.channel_index).cloned() {
                app.save_current_read_marker();
                app.current_server = Some(server.clone());
                app.current_channel = Some(target.clone());
                app.mark_target_read(&server, &target);
                app.user_list.clear();
                app.user_list_filter.clear();
                app.restore_read_marker_for(&server, &target);
                if target.starts_with('#') || target.starts_with('&') {
                    request_channel_names(clients, app);
                }
                app.panel_focus = PanelFocus::Main;
            }
        }
        ChannelSelectByNumber(n) => {
            let list = app.channels_list();
            let idx = if n == 10 { 9 } else { (n as usize).saturating_sub(1) };
            let idx = idx.min(list.len().saturating_sub(1));
            if idx < list.len() {
                app.channel_index = idx;
                if let Some((server, target)) = list.get(idx).cloned() {
                    app.save_current_read_marker();
                    app.current_server = Some(server.clone());
                    app.current_channel = Some(target.clone());
                    app.mark_target_read(&server, &target);
                    app.user_list.clear();
                    app.user_list_filter.clear();
                    app.restore_read_marker_for(&server, &target);
                    if target.starts_with('#') || target.starts_with('&') {
                        request_channel_names(clients, app);
                    }
                    app.panel_focus = PanelFocus::Main;
                }
            }
        }
        UserUp => {
            app.user_index = app.user_index.saturating_sub(1);
        }
        UserDown => {
            let len = app.filtered_user_list().len();
            if app.user_index + 1 < len {
                app.user_index += 1;
            }
        }
        UserSelect => {
            app.user_action_menu = true;
            app.user_action_index = 0;
        }
        UserSelectByNumber(n) => {
            let filtered = app.filtered_user_list();
            let idx = if n == 10 { 9 } else { (n as usize).saturating_sub(1) };
            let idx = idx.min(filtered.len().saturating_sub(1));
            if idx < filtered.len() {
                app.user_index = idx;
                app.user_action_menu = true;
                app.user_action_index = 0;
            }
        }
        UserListFilterFocus => {
            app.user_list_filter_focused = true;
        }
        UserListFilterUnfocus => {
            app.user_list_filter_focused = false;
        }
        UserListFilterChar(c) => {
            if !c.is_control() || c == ' ' {
                app.user_list_filter.push(c);
                app.clamp_user_index();
            }
        }
        UserListFilterBackspace => {
            if app.user_list_filter.pop().is_some() {
                app.clamp_user_index();
            }
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
                        app.input_cursor = app.input.len();
                        app.input_selection = None;
                    }
                    UserAction::Whois => {
                        app.user_action_menu = false;
                        if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
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
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
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
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
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
                    UserAction::Unban => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let mask = format!("{}!*@*", nick);
                                    let _ = c.send_mode(ch, &[IrcMode::Minus(IrcChannelMode::Ban, Some(mask))]);
                                    app.status_message = format!("Unbanned {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Op => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Plus(IrcChannelMode::Oper, Some(nick.clone()))]);
                                    app.status_message = format!("Opped {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Deop => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Minus(IrcChannelMode::Oper, Some(nick.clone()))]);
                                    app.status_message = format!("Deopped {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Voice => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Plus(IrcChannelMode::Voice, Some(nick.clone()))]);
                                    app.status_message = format!("Voiced {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Devoice => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Minus(IrcChannelMode::Voice, Some(nick.clone()))]);
                                    app.status_message = format!("Devoiced {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Halfop => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Plus(IrcChannelMode::Halfop, Some(nick.clone()))]);
                                    app.status_message = format!("Halfopped {} on {}", nick, ch);
                                }
                            } else {
                                app.status_message = "Not a channel.".to_string();
                            }
                        } else {
                            app.status_message = "No channel.".to_string();
                        }
                    }
                    UserAction::Dehalfop => {
                        app.user_action_menu = false;
                        if let Some(ref ch) = app.current_channel.as_ref() {
                            if (ch.starts_with('#') || ch.starts_with('&')) && app.current_server.as_ref().and_then(|s| clients.get(s)).is_some() {
                                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                                    let _ = c.send_mode(ch, &[IrcMode::Minus(IrcChannelMode::Halfop, Some(nick.clone()))]);
                                    app.status_message = format!("Dehalfopped {} on {}", nick, ch);
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
            if let Some((server, ch)) = app.selected_list_channel_and_server() {
                app.channel_list_popup_visible = false;
                app.channel_list_filter.clear();
                app.channel_list_scroll_mode = false;
                if let Some((ref c, _)) = clients.get(server.as_str()) {
                    c.send_join(&ch).map_err(|e| e.to_string())?;
                    let _ = c.send_topic(&ch, "");
                    let chans = app.channels_per_server.entry(server.clone()).or_default();
                    if !chans.contains(&ch) {
                        chans.push(ch.clone());
                    }
                    app.save_current_read_marker();
                    app.current_server = Some(server.clone());
                    app.current_channel = Some(ch.clone());
                    app.mark_target_read(&server, &ch);
                    app.sync_channel_index_to_current();
                    app.restore_read_marker_for(&server, &ch);
                    app.status_message = format!("Joined {} on {}", ch, server);
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
        DismissAwayPopup => {
            app.away_popup_visible = false;
            app.away_message = None;
            for server in &app.connected_servers {
                if let Some((ref c, _)) = clients.get(server.as_str()) {
                    let _ = c.send(IrcCommand::AWAY(None));
                }
            }
            app.status_message = "No longer away.".to_string();
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
                    if clients.contains_key(&name) {
                        app.status_message = format!("Already connected to {}.", name);
                    } else {
                        app.clear_reconnect();
                        let initial_away = app.away_message.clone();
                        match connect(server, config, irc_tx.clone(), rt, initial_away, &sts_policies) {
                            Ok((c, stream)) => {
                                let name_for_spawn = name.clone();
                                let host = server.host.clone();
                                let use_tls = sts_policies.get_valid(&server.host).is_some() || server.tls;
                                let tx = irc_tx.clone();
                                let our_nick = config.nickname.clone();
                                let handle = rt.spawn(async move {
                                    run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await;
                                });
                                clients.insert(name.clone(), (c, handle));
                                if app.current_server.is_none() {
                                    app.current_server = Some(name.clone());
                                    app.current_channel = Some("*server*".to_string());
                                    app.mark_target_read(&name, "*server*");
                                    app.channel_index = 0;
                                }
                                app.current_nickname = config.nickname.clone();
                                if server.sasl_mechanism.is_none() {
                                    if let Some(ref pw) = server.identify_password {
                                        if let Some((ref c, _)) = clients.get(name.as_str()) {
                                            let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                            app.status_message = "Identifying with NickServ...".to_string();
                                        }
                                        app.auto_join_after_per_server.insert(name.clone(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                                    }
                                }
                                app.status_message = format!("Connected to {}.", name);
                            }
                            Err(e) => {
                                app.status_message = e;
                            }
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
            let prev = app.message_scroll_offset;
            app.message_scroll_offset = app.message_scroll_offset.saturating_add(15);
            // Request CHATHISTORY BEFORE when user scrolls up past threshold (scroll-back).
            if prev >= 20
                && app.chathistory_before_pending.is_none()
                && app.current_server.is_some()
            {
                let target = app.current_channel.as_deref().unwrap_or("*server*");
                if target != "*server*" {
                    let server = app.current_server.as_ref().unwrap();
                    let target_key = app::msg_key(server, target);
                    let ref_str = app.current_messages()
                        .iter()
                        .filter(|m| !app.is_muted(&target_key, &m.source))
                        .next()
                        .and_then(|m| {
                            m.msgid.as_ref().map(|id| format!("msgid={}", id)).or_else(|| {
                                m.timestamp.as_ref().map(|t| format!("timestamp={}", t.to_rfc3339()))
                            })
                        });
                    if let Some(reference) = ref_str {
                        app.chathistory_before_pending = Some((server.clone(), target.to_string()));
                        let _ = irc_tx.send(IrcMessage::RequestChathistoryBefore {
                            server: server.clone(),
                            target: target.to_string(),
                            reference,
                        });
                    }
                }
            }
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
        SearchPopupUp => {
            app.search_selected_index = app.search_selected_index.saturating_sub(1);
        }
        SearchPopupDown => {
            if app.search_selected_index + 1 < app.search_results.len() {
                app.search_selected_index += 1;
            }
        }
        SearchPopupSelect => {
            if let Some((msg_index, _)) = app.search_results.get(app.search_selected_index) {
                let target_key = app.current_channel.as_deref().unwrap_or("*server*");
                let messages: Vec<_> = app
                    .current_messages()
                    .iter()
                    .filter(|m| !app.is_muted(target_key, &m.source))
                    .cloned()
                    .collect();
                if *msg_index < messages.len() {
                    let nick = app.current_nickname.as_deref();
                    const SEARCH_WIDTH: u16 = 72;
                    let item_heights: Vec<usize> = messages.iter().map(|m| {
                        let h = ui::message_wrapped_height(m, nick, SEARCH_WIDTH, &app.reactions) as usize;
                        match m.image_id {
                            Some(id) if app.inline_images.contains_key(&id) => h + crate::ui::IMAGE_DISPLAY_HEIGHT as usize,
                            Some(_) => h + 1,
                            None => h,
                        }
                    }).collect();
                    let rows_from_bottom: usize = item_heights[*msg_index + 1..].iter().sum();
                    app.message_scroll_offset = rows_from_bottom;
                }
                app.search_popup_visible = false;
                app.search_filter.clear();
                app.search_scroll_mode = false;
                app.status_message = "Jumped to message.".to_string();
            }
        }
        SearchPopupClose => {
            app.search_popup_visible = false;
            app.search_filter.clear();
            app.search_scroll_mode = false;
        }
        HighlightPopupClose => {
            app.highlight_popup_visible = false;
            app.highlight_input.clear();
        }
        HighlightPopupUp => {
            app.highlight_selected_index = app.highlight_selected_index.saturating_sub(1);
        }
        HighlightPopupDown => {
            if app.highlight_selected_index + 1 < app.highlight_words.len() {
                app.highlight_selected_index += 1;
            }
        }
        HighlightPopupAdd => {
            let word = app.highlight_input.trim().to_string();
            app.highlight_input.clear();
            if !word.is_empty() && !app.highlight_words.iter().any(|w| w.eq_ignore_ascii_case(&word)) {
                app.highlight_words.push(word.clone());
                app.highlight_words.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                if let Some(ref path) = app.highlight_path {
                    let _ = crate::highlight::save_highlights(path, &app.highlight_words);
                }
            }
        }
        HighlightPopupRemove => {
            if app.highlight_selected_index < app.highlight_words.len() {
                app.highlight_words.remove(app.highlight_selected_index);
                if app.highlight_selected_index >= app.highlight_words.len() && app.highlight_selected_index > 0 {
                    app.highlight_selected_index -= 1;
                }
                if let Some(ref path) = app.highlight_path {
                    let _ = crate::highlight::save_highlights(path, &app.highlight_words);
                }
            }
        }
        HighlightPopupInputChar(c) => {
            app.highlight_input.push(c);
        }
        HighlightPopupBackspace => {
            if !app.highlight_input.is_empty() {
                let i = app.highlight_input
                    .char_indices()
                    .rev()
                    .next()
                    .map(|(i, _)| i)
                    .unwrap_or(app.highlight_input.len());
                app.highlight_input.truncate(i);
            }
        }
        SearchPopupFocusList => {
            app.search_scroll_mode = true;
        }
        SearchPopupFocusFilter => {
            app.search_scroll_mode = false;
        }
        SearchPopupFilterChar(c) => {
            if c != '\0' {
                app.search_filter.push(c);
                app.update_search_results();
            }
        }
        SearchPopupBackspace => {
            if !app.search_filter.is_empty() {
                app.search_filter.pop();
                app.update_search_results();
            }
        }
        Char(c) => {
            if c != '\0' {
                if app.mode == Mode::Insert || app.mode == Mode::Command {
                    if app.input.len() >= format::MAX_INPUT_BYTES {
                        return Ok(false);
                    }
                    if let Some((start, end)) = app.input_selection.take() {
                        let lo = app.input.floor_char_boundary(start.min(end).min(app.input.len()));
                        let hi = app.input.ceil_char_boundary(start.max(end).min(app.input.len()));
                        app.input.replace_range(lo..hi, &c.to_string());
                        app.input_cursor = lo + c.len_utf8();
                    } else {
                        let len = app.input.len();
                        app.input_cursor = app.input.floor_char_boundary(app.input_cursor.min(len));
                        app.input.insert(app.input_cursor, c);
                        app.input_cursor += c.len_utf8();
                    }
                    if app.mode == Mode::Insert {
                        send_typing_indicator(app, clients, "active");
                    }
                }
            }
        }
        Paste(s) => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                // Filter control chars, keep printable; replace newlines with space for single-line IRC.
                let s: String = s
                    .chars()
                    .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
                    .filter(|c| !c.is_control() || *c == ' ')
                    .collect();
                if s.is_empty() {
                    return Ok(false);
                }
                let remaining = format::MAX_INPUT_BYTES.saturating_sub(app.input.len());
                let to_insert: &str = if s.len() <= remaining { &s } else { &s[..s.floor_char_boundary(remaining)] };
                if to_insert.is_empty() {
                    return Ok(false);
                }
                if let Some((start, end)) = app.input_selection.take() {
                    let lo = app.input.floor_char_boundary(start.min(end).min(app.input.len()));
                    let hi = app.input.ceil_char_boundary(start.max(end).min(app.input.len()));
                    app.input.replace_range(lo..hi, to_insert);
                    app.input_cursor = lo + to_insert.len();
                } else {
                    let len = app.input.len();
                    app.input_cursor = app.input.floor_char_boundary(app.input_cursor.min(len));
                    app.input.insert_str(app.input_cursor, to_insert);
                    app.input_cursor += to_insert.len();
                }
                if app.mode == Mode::Insert {
                    send_typing_indicator(app, clients, "active");
                }
            }
        }
        Backspace => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                if let Some((start, end)) = app.input_selection.take() {
                    let lo = app.input.floor_char_boundary(start.min(end).min(app.input.len()));
                    let hi = app.input.ceil_char_boundary(start.max(end).min(app.input.len()));
                    app.input.replace_range(lo..hi, "");
                    app.input_cursor = lo;
                } else {
                    let len = app.input.len();
                    app.input_cursor = app.input_cursor.min(len);
                    if app.input_cursor > 0 {
                        let prev = input_prev_char_boundary(&app.input, app.input_cursor);
                        app.input.replace_range(prev..app.input_cursor, "");
                        app.input_cursor = prev;
                    }
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
                    app.input_selection = None;
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
                app.input_selection = None;
            }
        }
        TabComplete => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                app.input_selection = None;
                complete_input(app);
            }
        }
        InputCursorLeft => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                app.input_selection = None;
                app.input_cursor = input_prev_char_boundary(&app.input, app.input_cursor);
            }
        }
        InputCursorRight => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                app.input_selection = None;
                app.input_cursor = input_next_char_boundary(&app.input, app.input_cursor);
            }
        }
        InputCursorHome => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                app.input_selection = None;
                app.input_cursor = 0;
            }
        }
        InputCursorEnd => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                app.input_selection = None;
                app.input_cursor = app.input.len();
            }
        }
        InputSelectLeft => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                let new_cursor = input_prev_char_boundary(&app.input, app.input_cursor);
                let (start, end) = app.input_selection
                    .map(|(s, e)| (s.min(e), s.max(e)))
                    .unwrap_or((app.input_cursor, app.input_cursor));
                let anchor = if (start, end) == (app.input_cursor, app.input_cursor) {
                    app.input_cursor
                } else if app.input_cursor == end {
                    start
                } else {
                    end
                };
                app.input_cursor = new_cursor;
                let (lo, hi) = (new_cursor.min(anchor), new_cursor.max(anchor));
                app.input_selection = if lo != hi { Some((lo, hi)) } else { None };
            }
        }
        InputSelectRight => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                let new_cursor = input_next_char_boundary(&app.input, app.input_cursor);
                let (start, end) = app.input_selection
                    .map(|(s, e)| (s.min(e), s.max(e)))
                    .unwrap_or((app.input_cursor, app.input_cursor));
                let anchor = if (start, end) == (app.input_cursor, app.input_cursor) {
                    app.input_cursor
                } else if app.input_cursor == start {
                    end
                } else {
                    start
                };
                app.input_cursor = new_cursor;
                let (lo, hi) = (new_cursor.min(anchor), new_cursor.max(anchor));
                app.input_selection = if lo != hi { Some((lo, hi)) } else { None };
            }
        }
        InputSelectHome => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                let new_cursor = 0;
                let (start, end) = app.input_selection
                    .map(|(s, e)| (s.min(e), s.max(e)))
                    .unwrap_or((app.input_cursor, app.input_cursor));
                let anchor = if (start, end) == (app.input_cursor, app.input_cursor) {
                    app.input_cursor
                } else if app.input_cursor == end { start } else { end };
                app.input_cursor = new_cursor;
                let (lo, hi) = (new_cursor.min(anchor), new_cursor.max(anchor));
                app.input_selection = if lo != hi { Some((lo, hi)) } else { None };
            }
        }
        InputSelectEnd => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                let new_cursor = app.input.len();
                let (start, end) = app.input_selection
                    .map(|(s, e)| (s.min(e), s.max(e)))
                    .unwrap_or((app.input_cursor, app.input_cursor));
                let anchor = if (start, end) == (app.input_cursor, app.input_cursor) {
                    app.input_cursor
                } else if app.input_cursor == start { end } else { start };
                app.input_cursor = new_cursor;
                let (lo, hi) = (new_cursor.min(anchor), new_cursor.max(anchor));
                app.input_selection = if lo != hi { Some((lo, hi)) } else { None };
            }
        }
        InputDelete => {
            if app.mode == Mode::Insert || app.mode == Mode::Command {
                if let Some((start, end)) = app.input_selection.take() {
                    let lo = app.input.floor_char_boundary(start.min(end).min(app.input.len()));
                    let hi = app.input.ceil_char_boundary(start.max(end).min(app.input.len()));
                    app.input.replace_range(lo..hi, "");
                    app.input_cursor = lo;
                } else {
                    let len = app.input.len();
                    app.input_cursor = app.input.floor_char_boundary(app.input_cursor.min(len));
                    if app.input_cursor < len {
                        let next = input_next_char_boundary(&app.input, app.input_cursor);
                        app.input.replace_range(app.input_cursor..next, "");
                    }
                }
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
                app.input_selection = None;
                if text.starts_with(':') {
                    if run_command(app, clients, config, irc_tx, rt, &sts_policies, &sts_path, &text)? {
                        return Ok(true);
                    }
                } else if let (Some(server), Some((ref c, _))) = (app.current_server.clone(), app.current_server.as_ref().and_then(|s| clients.get(s.as_str()))) {
                    let target = app.current_channel.as_deref().unwrap_or("*").to_string();
                    if target == "*server*" {
                        app.status_message = "Cannot send to server.".to_string();
                    } else if !text.is_empty() {
                        send_typing_indicator(app, clients, "done");
                        let formatted = format::format_outgoing(&text);
                        let reply_to = app.reply_to_msgid.take();
                        let sec_key = app::msg_key(&server, &target);
                        if app.secure_sessions.contains_key(&sec_key) {
                            let session = app.secure_sessions.get_mut(&sec_key).unwrap();
                            let mut ok = true;
                            for chunk in format::split_message_for_irc(&formatted, format::MAX_ENCRYPTED_PLAINTEXT_BYTES) {
                                match session.encrypt(&chunk) {
                                    Ok((nonce, ct)) => {
                                        let wire = format!("[:rvIRC:ENC:{}:{}]", nonce, ct);
                                        c.send_privmsg(&target, &wire).map_err(|e| e.to_string())?;
                                    }
                                    Err(e) => {
                                        app.status_message = format!("Encrypt error: {}", e);
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            if ok {
                                if app.away_message.is_some() {
                                    let _ = c.send(IrcCommand::AWAY(None));
                                    app.away_message = None;
                                    app.status_message = "Auto-unaway.".to_string();
                                }
                                push_self_message(app, &server, &target, formatted, reply_to.clone(), irc_tx, rt);
                            }
                        } else {
                            let mut first = true;
                            for chunk in format::split_message_for_irc(&formatted, format::MAX_MESSAGE_BYTES) {
                                if first && reply_to.is_some() {
                                    let tags = Some(vec![Tag("+reply".to_string(), reply_to.clone())]);
                                    let msg = IrcProtoMessage::with_tags(
                                        tags,
                                        None,
                                        "PRIVMSG",
                                        vec![target.as_str(), chunk.as_str()],
                                    )
                                    .map_err(|e| e.to_string())?;
                                    c.send(msg).map_err(|e| e.to_string())?;
                                } else {
                                    c.send_privmsg(&target, &chunk).map_err(|e| e.to_string())?;
                                }
                                first = false;
                            }
                            if app.away_message.is_some() {
                                let _ = c.send(IrcCommand::AWAY(None));
                                app.away_message = None;
                                app.status_message = "Auto-unaway.".to_string();
                            }
                            push_self_message(app, &server, &target, formatted, reply_to.clone(), irc_tx, rt);
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
                app.input_cursor = 0;
                app.input_selection = None;
                app.mode = Mode::Normal;
                if run_command(app, clients, config, irc_tx, rt, &sts_policies, &sts_path, &line)? {
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
                    let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                    app.push_chat_log(&server, &nick, "Secure handshake failed: invalid identity key.");
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
                    app.secure_sessions.insert(app::msg_key(&server, &nick), session);
                    if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                        let msg = format!("[:rvIRC:SECURE:ACK:{}:{}]", our_ephemeral_pub_b64, our_identity_pub_b64);
                        let _ = c.send_privmsg(&nick, &msg);
                    }
                    let fp = key_fingerprint(&identity_b64);
                    app.push_chat_log(&server, &nick, &format!("Key fingerprint: {}", fp));
                    app.push_chat_log(&server, &nick, "*** SECURE CONNECTION ESTABLISHED ***");
                    app.push_chat_log(&server, &nick, "Messages are now end-to-end encrypted (X25519 + ChaCha20-Poly1305).");
                    if !app.known_keys.is_verified(&nick, &server) {
                        app.push_chat_log(&server, &nick, "Use :verify to compare verification codes.");
                    }
                    app.status_message = format!("Secure session established with {}.", nick);
                }
                Err(e) => {
                    app.push_chat_log(&server, &nick, &format!("Secure handshake failed: {}", e));
                    app.status_message = format!("Secure handshake from {} failed: {}", nick, e);
                }
            }
        }
        SecureReject => {
            let nick = app.secure_accept_nick.clone();
            let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
            app.secure_accept_popup_visible = false;
            app.push_chat_log(&server, &nick, "*** Secure session request rejected. ***");
            app.status_message = format!("Rejected secure session from {}.", nick);
        }
        FileReceiveAccept => {
            let code = app.file_receive_code.clone();
            let filename = app.file_receive_filename.clone();
            let nick = app.file_receive_nick.clone();
            let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
            app.file_receive_popup_visible = false;

            app.push_chat_log(&server, &nick, &format!("Accepted file: {}", filename));

            if let Some(dl_dir) = config.resolved_download_dir() {
                let safe_name = sanitize_received_filename(&filename);
                let save_path = dl_dir.join(&safe_name);
                let tx = irc_tx.clone();
                let nick_c = nick.clone();
                let server_c = server.clone();
                app.push_chat_log(&server, &nick, &format!("Saving to {}...", save_path.display()));
                app.status_message = format!("Receiving {} from {}...", filename, nick);
                rt.spawn(async move {
                    match filetransfer::receive_file(&code, &save_path, &server_c, &nick_c, &tx).await {
                        Ok(()) => {
                            let _ = tx.send(IrcMessage::SendPrivmsg {
                                server: server_c.clone(),
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
                                server: server_c,
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
            if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
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
                    let server = app.current_server.clone().unwrap_or_else(|| "unknown".to_string());
                    app.file_browser_visible = false;

                    let tx = irc_tx.clone();
                    let nick_clone = nick.clone();
                    let server_clone = server.clone();
                    app.push_chat_log(&server, &nick, &format!("Starting file send: {}", name));
                    app.status_message = format!("Starting file send of {} to {}...", name, nick);
                    rt.spawn(async move {
                        match filetransfer::send_file(&file_path, server_clone.clone(), nick_clone.clone(), tx.clone()).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = tx.send(IrcMessage::ChatLog {
                                    server: server_clone,
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
                let server = app.current_server.clone().unwrap_or_else(|| "unknown".to_string());
                app.file_browser_visible = false;

                let safe_name = sanitize_received_filename(&filename);
                let save_path = save_dir.join(&safe_name);
                let tx = irc_tx.clone();
                let nick_c = nick.clone();
                let server_c = server.clone();
                app.push_chat_log(&server, &nick, &format!("Receiving {} to {}...", filename, save_path.display()));
                app.status_message = format!("Receiving {} from {}...", filename, nick);
                rt.spawn(async move {
                    match filetransfer::receive_file(&code, &save_path, &server_c, &nick_c, &tx).await {
                        Ok(()) => {
                            let _ = tx.send(IrcMessage::SendPrivmsg {
                                server: server_c.clone(),
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
                                server: server_c,
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
            if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                let _ = c.send_privmsg(&nick, "[:rvIRC:WORMHOLE:REJECT]");
            }
            app.status_message = "File transfer cancelled.".to_string();
        }
        Esc => {
            if app.mode == Mode::Insert {
                send_typing_indicator(app, clients, "done");
                app.reply_to_msgid = None;
            }
            app.mode = Mode::Normal;
            app.input.clear();
            app.input_cursor = 0;
            app.input_selection = None;
            app.user_action_menu = false;
            app.panel_focus = PanelFocus::Main;
        }
    }
    Ok(false)
}

/// Returns Ok(true) if the program should exit (e.g. after :quit / :q).
fn run_command(
    app: &mut App,
    clients: &mut HashMap<String, (Client, tokio::task::JoinHandle<()>)>,
    config: &RvConfig,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
    sts_policies: &sts::StsPolicies,
    _sts_path: &std::path::Path,
    line: &str,
) -> Result<bool, String> {
    let line = line.trim_start_matches(':');
    let result = commands::parse(line);
    use commands::CommandResult as R;
    match result {
        R::Join { channel: ch, key } => {
            if let Some(server) = app.current_server.clone() {
                if let Some((ref c, _)) = clients.get(server.as_str()) {
                    if let Some(ref k) = key {
                        c.send_join_with_keys(&ch, k).map_err(|e| e.to_string())?;
                    } else {
                        c.send_join(&ch).map_err(|e| e.to_string())?;
                    }
                    let _ = c.send_topic(&ch, "");
                    let chans = app.channels_per_server.entry(server.to_string()).or_default();
                    if !chans.contains(&ch) {
                        chans.push(ch.clone());
                    }
                    app.save_current_read_marker();
                    app.current_channel = Some(ch.clone());
                    app.mark_target_read(&server, &ch);
                    app.sync_channel_index_to_current();
                    app.restore_read_marker_for(&server, &ch);
                    app.status_message = format!("Joined {}", ch);
                } else {
                    app.status_message = "Not connected.".to_string();
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Part(Some(target)) => {
            if let Some(server) = app.current_server.as_ref() {
                if target.starts_with('#') || target.starts_with('&') {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        c.send_part(&target).map_err(|e| e.to_string())?;
                        if let Some(chans) = app.channels_per_server.get_mut(server) {
                            chans.retain(|x| x != &target);
                        }
                    }
                    app.clamp_channel_index();
                    app.save_current_read_marker();
                    if let Some((s, t)) = app.selected_channel_entry().or_else(|| app.selected_message_entry()) {
                        app.current_server = Some(s.clone());
                        app.current_channel = Some(t.clone());
                        app.restore_read_marker_for(&s, &t);
                    }
                } else {
                    // DM: close the message window
                    if let Some(dms) = app.dm_targets_per_server.get_mut(server) {
                        if dms.contains(&target) {
                            dms.retain(|x| x != &target);
                            app.clamp_messages_index();
                            if app.current_channel.as_ref() == Some(&target) {
                                app.save_current_read_marker();
                                if let Some((s, t)) = app.messages_list().first().cloned()
                                    .or_else(|| app.channels_list().first().cloned()) {
                                    app.current_server = Some(s.clone());
                                    app.current_channel = Some(t.clone());
                                    app.sync_channel_index_to_current();
                                    app.restore_read_marker_for(&s, &t);
                                }
                            }
                            app.status_message = format!("Closed DM with {}", target);
                        } else {
                            app.status_message = format!("No DM with {}.", target);
                        }
                    }
                }
            }
            if let (Some(s), Some(t)) = (app.current_server.clone(), app.current_channel.clone()) {
                app.mark_target_read(&s, &t);
            }
        }
        R::Part(None) => {
            if let (Some(server), Some(target)) = (app.current_server.as_ref(), app.current_channel.clone()) {
                if target.starts_with('#') || target.starts_with('&') {
                    if app.channels_per_server.get(server).map_or(false, |chans| chans.contains(&target)) {
                        if let Some((ref c, _)) = clients.get(server.as_str()) {
                            c.send_part(&target).map_err(|e| e.to_string())?;
                        }
                        if let Some(chans) = app.channels_per_server.get_mut(server) {
                            chans.retain(|x| x != &target);
                        }
                    }
                    app.clamp_channel_index();
                    app.save_current_read_marker();
                    if let Some((s, t)) = app.selected_channel_entry().or_else(|| app.selected_message_entry()) {
                        app.current_server = Some(s.clone());
                        app.current_channel = Some(t.clone());
                        app.restore_read_marker_for(&s, &t);
                    }
                } else {
                    if let Some(dms) = app.dm_targets_per_server.get_mut(server) {
                        if dms.contains(&target) {
                            dms.retain(|x| x != &target);
                            app.clamp_messages_index();
                            app.save_current_read_marker();
                            if let Some((s, t)) = app.messages_list().first().cloned()
                                .or_else(|| app.channels_list().first().cloned()) {
                                app.current_server = Some(s.clone());
                                app.current_channel = Some(t.clone());
                            }
                            app.sync_channel_index_to_current();
                            if let (Some(s), Some(t)) = (app.current_server.clone(), app.current_channel.clone()) {
                                app.restore_read_marker_for(&s, &t);
                            }
                            app.status_message = format!("Closed DM with {}", target);
                        }
                    }
                }
                if let (Some(s), Some(t)) = (app.current_server.clone(), app.current_channel.clone()) {
                    app.mark_target_read(&s, &t);
                }
            }
        }
        R::List(server_arg) => {
            let server = server_arg
                .as_ref()
                .and_then(|name| {
                    app.connected_servers
                        .iter()
                        .find(|s| s.eq_ignore_ascii_case(name))
                        .cloned()
                })
                .or_else(|| app.current_server.clone());
            if let (Some(server_name), Some((ref c, _))) =
                (server.clone(), server.as_ref().and_then(|s| clients.get(s.as_str())))
            {
                let _ = c.send(IrcCommand::LIST(None, None));
                app.channel_list_popup_visible = true;
                app.server_channel_list = Vec::new();
                app.channel_list_filter.clear();
                app.channel_list_selected_index = 0;
                app.channel_list_scroll_mode = false;
                app.channel_list_server = Some(server_name.clone());
                app.channel_list_super = false;
                app.channel_list_pending_servers.clear();
                app.status_message = format!("Fetching channel list from {}...", server_name);
            } else if server_arg.as_ref().is_some() {
                app.status_message = format!(
                    "Server not connected. Connected: {}",
                    app.connected_servers.join(", ")
                );
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::SuperList => {
            if app.connected_servers.is_empty() {
                app.status_message = "Not connected.".to_string();
            } else {
                for server in &app.connected_servers {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let _ = c.send(IrcCommand::LIST(None, None));
                    }
                }
                app.channel_list_popup_visible = true;
                app.server_channel_list = Vec::new();
                app.channel_list_filter.clear();
                app.channel_list_selected_index = 0;
                app.channel_list_scroll_mode = false;
                app.channel_list_server = None;
                app.channel_list_super = true;
                app.channel_list_pending_servers = app.connected_servers.iter().cloned().collect();
                app.status_message = "Fetching channel list from all servers...".to_string();
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
                    if let Some((c, h)) = clients.remove(&server_name) {
                        let _ = c.send_quit("Reconnecting");
                        h.abort();
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    let initial_away = app.away_message.clone();
                    match connect(server, config, irc_tx.clone(), rt, initial_away, &sts_policies) {
                        Ok((c, stream)) => {
                            let host = server.host.clone();
                            let use_tls = sts_policies.get_valid(&server.host).is_some() || server.tls;
                            let tx = irc_tx.clone();
                            let our_nick = config.nickname.clone();
                            let name_for_spawn = server_name.clone();
                            let handle = rt.spawn(async move { run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await });
                            clients.insert(server_name.clone(), (c, handle));
                            app.current_nickname = config.nickname.clone();
                            app.current_channel = Some("*server*".to_string());
                            app.mark_target_read(&server_name, "*server*");
                            app.channel_index = 0;
                            if server.sasl_mechanism.is_none() {
                                if let Some(ref pw) = server.identify_password {
                                    if let Some((ref c, _)) = clients.get(server_name.as_str()) {
                                        let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                        app.status_message = "Identifying with NickServ...".to_string();
                                    }
                                    app.auto_join_after_per_server.insert(server_name.clone(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                                }
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
                    if clients.contains_key(&name) {
                        app.status_message = format!("Already connected to {}.", name);
                    } else {
                        app.clear_reconnect();
                        let initial_away = app.away_message.clone();
                        match connect(server, config, irc_tx.clone(), rt, initial_away, &sts_policies) {
                            Ok((c, stream)) => {
                                let host = server.host.clone();
                                let use_tls = sts_policies.get_valid(&server.host).is_some() || server.tls;
                                let tx = irc_tx.clone();
                                let our_nick = config.nickname.clone();
                                let name_for_spawn = name.clone();
                                let handle = rt.spawn(async move {
                                    run_stream(stream, tx, name_for_spawn, host, use_tls, our_nick).await;
                                });
                                clients.insert(name.clone(), (c, handle));
                                if app.current_server.is_none() {
                                    app.current_server = Some(name.clone());
                                    app.current_channel = Some("*server*".to_string());
                                    app.mark_target_read(&name, "*server*");
                                    app.channel_index = 0;
                                }
                                app.current_nickname = config.nickname.clone();
                                if server.sasl_mechanism.is_none() {
                                    if let Some(ref pw) = server.identify_password {
                                        if let Some((ref c, _)) = clients.get(name.as_str()) {
                                            let _ = c.send_privmsg("NickServ", &format!("IDENTIFY {}", pw));
                                            app.status_message = "Identifying with NickServ...".to_string();
                                        }
                                        app.auto_join_after_per_server.insert(name.clone(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                                    }
                                }
                                app.status_message = format!("Connected to {}.", name);
                            }
                            Err(e) => {
                                app.status_message = e;
                            }
                        }
                    }
                }
            }
        }
        R::Disconnect(server_arg) => {
            let server = server_arg.or_else(|| app.current_server.clone());
            if let Some(server) = server {
                app.clear_reconnect();
                if let Some((c, h)) = clients.remove(&server) {
                    let _ = c.send_quit("Leaving");
                    h.abort();
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                app.connected_servers.retain(|s| s != &server);
                app.channels_per_server.remove(&server);
                app.dm_targets_per_server.remove(&server);
                app.typing_status.retain(|(s, _, _), _| s != &server);
                app.last_typing_sent.retain(|k, _| !k.starts_with(&format!("{}/", server)));
                for k in app.messages.keys().cloned().collect::<Vec<_>>() {
                    if k.starts_with(&format!("{}/", server)) {
                        app.messages.remove(&k);
                    }
                }
                app.unread_targets.retain(|k| !k.starts_with(&format!("{}/", server)));
                app.unread_mentions.retain(|k| !k.starts_with(&format!("{}/", server)));
                app.current_server = app.connected_servers.first().cloned();
                app.current_channel = app.current_server.as_ref().map(|_| "*server*".to_string());
                app.sync_channel_index_to_current();
                app.clamp_channel_index();
                app.clamp_messages_index();
                app.user_list.clear();
                app.search_popup_visible = false;
                app.channel_list_popup_visible = false;
                app.server_channel_list.clear();
                app.channel_list_filter.clear();
                app.channel_list_scroll_mode = false;
                app.channel_list_server = None;
                app.channel_list_super = false;
                app.channel_list_pending_servers.clear();
                app.server_list_popup_visible = false;
                app.highlight_popup_visible = false;
                app.whois_popup_visible = false;
                app.whois_nick.clear();
                app.whois_lines.clear();
                app.pending_auto_join_servers.remove(&server);
                app.auto_join_after_per_server.remove(&server);
                app.friends_online.clear();
                app.friends_away.clear();
                app.away_message = None;
                app.away_popup_visible = false;
                app.status_message = "Disconnected. Type :connect <server> to reconnect.".to_string();
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Quit(_) => {
            app.clear_reconnect();
            for (_, (c, h)) in std::mem::take(clients) {
                let _ = c.send_quit("Leaving");
                h.abort();
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
            app.current_server = None;
            app.current_channel = None;
            app.connected_servers.clear();
            app.pending_auto_join_servers.clear();
            app.auto_join_after_per_server.clear();
            app.channels_per_server.clear();
            app.dm_targets_per_server.clear();
            app.unread_targets.clear();
            app.unread_mentions.clear();
            app.typing_status.clear();
            app.last_typing_sent.clear();
            app.user_list.clear();
            app.status_message = "Disconnected.".to_string();
            return Ok(true);
        }
        R::Msg { nick, text } => {
            if let (Some(server), Some((ref c, _))) = (app.current_server.clone(), app.current_server.as_ref().and_then(|s| clients.get(s.as_str()))) {
                if !text.is_empty() {
                    let formatted = format::format_outgoing(&text);
                    let sec_key = app::msg_key(&server, &nick);
                    if app.secure_sessions.contains_key(&sec_key) {
                        let session = app.secure_sessions.get_mut(&sec_key).unwrap();
                        let mut ok = true;
                        for chunk in format::split_message_for_irc(&formatted, format::MAX_ENCRYPTED_PLAINTEXT_BYTES) {
                            match session.encrypt(&chunk) {
                                Ok((nonce, ct)) => {
                                    let wire = format!("[:rvIRC:ENC:{}:{}]", nonce, ct);
                                    c.send_privmsg(&nick, &wire).map_err(|e| e.to_string())?;
                                }
                                Err(e) => {
                                    app.status_message = format!("Encrypt error: {}", e);
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            push_self_message(app, &server, &nick, formatted, None, irc_tx, rt);
                        }
                    } else {
                        for chunk in format::split_message_for_irc(&formatted, format::MAX_MESSAGE_BYTES) {
                            c.send_privmsg(&nick, &chunk).map_err(|e| e.to_string())?;
                        }
                        push_self_message(app, &server, &nick, formatted, None, irc_tx, rt);
                    }
                }
                let dms = app.dm_targets_per_server.entry(server.clone()).or_default();
                if !dms.contains(&nick) {
                    dms.push(nick.clone());
                }
                app.save_current_read_marker();
                app.current_channel = Some(nick.clone());
                app.mark_target_read(&server, &nick);
                app.sync_channel_index_to_current();
                app.restore_read_marker_for(&server, &nick);
                app.status_message = format!("Message sent to {}", nick);
            }
        }
        R::Me(text) => {
            if let (Some((ref c, _)), Some(ref target)) = (app.current_server.as_ref().and_then(|s| clients.get(s)), app.current_channel.as_ref()) {
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
            if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                let _ = c.send(IrcCommand::NICK(newnick.clone()));
                app.current_nickname = Some(newnick.clone());
                app.status_message = format!("Changing nick to {}", newnick);
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Topic(Some(topic)) => {
            if let (Some(server), Some((ref c, _)), Some(ref ch)) = (app.current_server.as_ref(), app.current_server.as_ref().and_then(|s| clients.get(s)), app.current_channel.as_ref()) {
                if ch.starts_with('#') || ch.starts_with('&') {
                    c.send_topic(ch, &topic).map_err(|e| e.to_string())?;
                    app.channel_topics.insert(app::msg_key(server, ch), topic.clone());
                    app.status_message = "Topic set.".to_string();
                } else {
                    app.status_message = "Not a channel.".to_string();
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Topic(None) => {
            if let (Some(ref server), Some(ref ch)) = (app.current_server.as_ref(), app.current_channel.as_ref()) {
                if ch.starts_with('#') || ch.starts_with('&') {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let _ = c.send_topic(ch, "");
                    }
                    if let Some(t) = app.channel_topics.get(&app::msg_key(server, ch)) {
                        app.status_message = if t.is_empty() { "No topic set.".to_string() } else { t.clone() };
                    } else {
                        app.status_message = "Requesting topic...".to_string();
                    }
                }
            }
        }
        R::Kick { channel, nick, reason } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some((ref c, _)), Some(ch)) = (app.current_server.as_ref().and_then(|s| clients.get(s)), ch) {
                c.send_kick(&ch, &nick, reason.as_deref().unwrap_or("")).map_err(|e| e.to_string())?;
                app.status_message = format!("Kicked {} from {}", nick, ch);
            } else {
                app.status_message = "Usage: :kick [channel] <nick> [reason]".to_string();
            }
        }
        R::Ban { channel, mask } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some((ref c, _)), Some(ch)) = (app.current_server.as_ref().and_then(|s| clients.get(s)), ch) {
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
        R::Unban { channel, mask } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some((ref c, _)), Some(ch)) = (app.current_server.as_ref().and_then(|s| clients.get(s)), ch) {
                if mask.is_empty() {
                    app.status_message = "Usage: :unban [channel] <mask>".to_string();
                } else {
                    c.send_mode(&ch, &[IrcMode::Minus(IrcChannelMode::Ban, Some(mask.clone()))]).map_err(|e| e.to_string())?;
                    app.status_message = format!("Unbanned {} on {}", mask, ch);
                }
            } else {
                app.status_message = "Usage: :unban [channel] <mask>".to_string();
            }
        }
        R::Invite { nick, channel } => {
            let ch = channel.or_else(|| app.current_channel.clone()).filter(|c| c.starts_with('#') || c.starts_with('&'));
            if let (Some((ref c, _)), Some(ch)) = (app.current_server.as_ref().and_then(|s| clients.get(s)), ch) {
                c.send_invite(&nick, &ch).map_err(|e| e.to_string())?;
                app.status_message = format!("Invited {} to {}", nick, ch);
            } else {
                app.status_message = "Usage: :invite <nick> [#channel] (need a channel)".to_string();
            }
        }
        R::Away(msg) => {
            if app.connected_servers.is_empty() {
                app.status_message = "Not connected.".to_string();
            } else if let Some(ref msg) = msg {
                for server in &app.connected_servers {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let _ = c.send(IrcCommand::AWAY(Some(msg.clone())));
                    }
                }
                app.away_message = Some(msg.clone());
                app.away_popup_visible = true;
            } else {
                for server in &app.connected_servers {
                    if let Some((ref c, _)) = clients.get(server.as_str()) {
                        let _ = c.send(IrcCommand::AWAY(None));
                    }
                }
                app.away_message = None;
                app.away_popup_visible = false;
                app.status_message = "No longer away.".to_string();
            }
        }
        R::SwitchChannel(ch) => {
            app.save_current_read_marker();
            app.current_channel = Some(ch.clone());
            if let Some(server) = app.current_server.clone() {
                app.mark_target_read(&server, &ch);
                app.sync_channel_index_to_current();
                app.restore_read_marker_for(&server, &ch);
            }
        }
        R::Highlight => {
            app.highlight_popup_visible = true;
            app.highlight_input.clear();
            app.highlight_selected_index = 0;
        }
        R::Search => {
            app.search_popup_visible = true;
            app.search_filter.clear();
            app.search_scroll_mode = false;
            app.update_search_results();
            app.search_selected_index = 0;
            app.status_message = "Search (type to filter, Enter to browse, Esc to close)".to_string();
        }
        R::Reply => {
            let ids = app.replyable_msgids();
            if ids.is_empty() {
                app.status_message = "No message with reply support in this buffer.".to_string();
            } else {
                app.mode = crate::app::Mode::Normal;
                app.reply_select_mode = true;
                app.status_message = "Press 1–9 or 0 to pick message (Esc to cancel).".to_string();
            }
        }
        R::Redact { msgid: cmd_msgid, reason } => {
            let msgid = cmd_msgid.or_else(|| app.reply_to_msgid.clone()).or_else(|| {
                let target_key = app::msg_key(
                    app.current_server.as_deref().unwrap_or(""),
                    app.current_channel.as_deref().unwrap_or("*server*"),
                );
                app.current_messages()
                    .iter()
                    .rev()
                    .find(|m| m.msgid.is_some() && !app.is_muted(&target_key, &m.source))
                    .and_then(|m| m.msgid.clone())
            });
            if let (Some(msgid), Some(server), Some((c, _))) = (
                msgid,
                app.current_server.as_ref(),
                app.current_server.as_ref().and_then(|s| clients.get(s.as_str())),
            ) {
                let target = app.current_channel.as_deref().unwrap_or("*server*");
                if target == "*server*" {
                    app.status_message = "Cannot redact in server buffer.".to_string();
                } else if !app.acked_caps_per_server.get(server).map_or(false, |s| s.contains("draft/message-redaction")) {
                    app.status_message = "Server does not support message redaction.".to_string();
                } else {
                    let mut args = vec![target.to_string(), msgid.clone()];
                    if let Some(r) = reason {
                        args.push(r);
                    }
                    if let Err(e) = c.send(IrcCommand::Raw("REDACT".to_string(), args)) {
                        app.status_message = format!("Redact failed: {}", e);
                    }
                }
            } else {
                app.status_message = "Select a message with r first, or :redact <msgid>.".to_string();
            }
        }
        R::React(emoji) => {
            let msgid = app.reply_to_msgid.clone().or_else(|| {
                let target_key = app::msg_key(
                    app.current_server.as_deref().unwrap_or(""),
                    app.current_channel.as_deref().unwrap_or("*server*"),
                );
                app.current_messages()
                    .iter()
                    .rev()
                    .find(|m| m.msgid.is_some() && !app.is_muted(&target_key, &m.source))
                    .and_then(|m| m.msgid.clone())
            });
            if let (Some(msgid), Some(_server), Some((c, _))) = (
                msgid,
                app.current_server.as_ref(),
                app.current_server.as_ref().and_then(|s| clients.get(s.as_str())),
            ) {
                let target = app.current_channel.as_deref().unwrap_or("*server*");
                if target == "*server*" {
                    app.status_message = "Cannot react in server buffer.".to_string();
                } else {
                    let tags = vec![
                        Tag("+reply".to_string(), Some(msgid)),
                        Tag("+draft/react".to_string(), Some(emoji)),
                    ];
                    if let Ok(msg) = IrcProtoMessage::with_tags(Some(tags), None, "TAGMSG", vec![target]) {
                        if let Err(e) = c.send(msg) {
                            app.status_message = format!("Reaction failed: {}", e);
                        }
                    }
                }
            } else {
                app.status_message = "Select a message with r first, or :reply.".to_string();
            }
        }
        R::FetchMoreHistory => {
            if app.chathistory_before_pending.is_some() {
                app.status_message = "Already fetching history.".to_string();
            } else if let Some(ref server) = app.current_server {
                let target = app.current_channel.as_deref().unwrap_or("*server*");
                if target == "*server*" {
                    app.status_message = "Cannot fetch history for server buffer.".to_string();
                } else {
                    let target_key = app::msg_key(server, target);
                    let ref_str = app.current_messages()
                        .iter()
                        .filter(|m| !app.is_muted(&target_key, &m.source))
                        .next()
                        .and_then(|m| {
                            m.msgid.as_ref().map(|id| format!("msgid={}", id)).or_else(|| {
                                m.timestamp.as_ref().map(|t| format!("timestamp={}", t.to_rfc3339()))
                            })
                        });
                    if let Some(reference) = ref_str {
                        app.chathistory_before_pending = Some((server.clone(), target.to_string()));
                        let _ = irc_tx.send(IrcMessage::RequestChathistoryBefore {
                            server: server.clone(),
                            target: target.to_string(),
                            reference,
                        });
                        app.status_message = "Fetching older messages...".to_string();
                    } else {
                        app.status_message = "No message to use as reference (need msgid or timestamp).".to_string();
                    }
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Clear => {
            let key = app.current_server.as_ref().map_or_else(|| "*server*".to_string(), |s| app::msg_key(s, app.current_channel.as_deref().unwrap_or("*server*")));
            let image_ids: Vec<usize> = app.messages
                .get(&key)
                .map(|v| v.iter().filter_map(|m| m.image_id).collect())
                .unwrap_or_default();
            for id in image_ids {
                app.inline_images.remove(&id);
            }
            app.messages.remove(&key);
            app.message_scroll_offset = 0;
            app.status_message = format!("Cleared {}.", key);
        }
        R::StatusMessage(m) => app.status_message = m,
        R::ChannelPanelShow => app.channel_panel_visible = true,
        R::ChannelPanelHide => {
            app.channel_panel_visible = false;
            if app.panel_focus == PanelFocus::Channels {
                app.panel_focus = PanelFocus::Main;
            }
        }
        R::MessagesPanelShow => app.messages_panel_visible = true,
        R::MessagesPanelHide => {
            app.messages_panel_visible = false;
            if app.panel_focus == PanelFocus::Messages {
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
        R::FriendsPanelShow => app.friends_panel_visible = true,
        R::FriendsPanelHide => {
            app.friends_panel_visible = false;
            if app.panel_focus == PanelFocus::Friends {
                app.panel_focus = PanelFocus::Main;
            }
        }
        R::FocusChannels => {
            if app.channel_panel_visible {
                app.panel_focus = PanelFocus::Channels;
            }
        }
        R::FocusMessages => {
            if app.messages_panel_visible {
                app.panel_focus = PanelFocus::Messages;
            }
        }
        R::FocusFriends => {
            if app.friends_panel_visible {
                app.panel_focus = PanelFocus::Friends;
            }
        }
        R::AddFriend(nick) => {
            let nick = nick.trim().to_string();
            if nick.is_empty() {
                app.status_message = "Usage: :add-friend <nick>".to_string();
            } else if app.friends_list.iter().any(|n| n.eq_ignore_ascii_case(&nick)) {
                app.status_message = format!("{} is already a friend.", nick);
            } else {
                app.friends_list.push(nick.clone());
                app.friends_list.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
                app.clamp_friends_index();
                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                    let _ = c.send(IrcCommand::MONITOR("+".to_string(), Some(nick.clone())));
                }
                if let Some(ref path) = app.friends_path {
                    let _ = crate::friends::save_friends(path, app.current_server.as_deref(), &app.friends_list);
                }
                app.status_message = format!("Added {} to friends.", nick);
            }
        }
        R::RemoveFriend(nick) => {
            let nick = nick.trim();
            if let Some(pos) = app.friends_list.iter().position(|n| n.eq_ignore_ascii_case(nick)) {
                app.friends_list.remove(pos);
                app.clamp_friends_index();
                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                    let _ = c.send(IrcCommand::MONITOR("-".to_string(), Some(nick.to_string())));
                }
                app.friends_online.remove(nick);
                app.friends_away.remove(nick);
                if let Some(ref path) = app.friends_path {
                    let _ = crate::friends::save_friends(path, app.current_server.as_deref(), &app.friends_list);
                }
                app.status_message = format!("Removed {} from friends.", nick);
            } else {
                app.status_message = format!("{} is not in friends list.", nick);
            }
        }
        R::Ignore(nick) => {
            let nick = nick.trim();
            if !nick.is_empty() {
                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                    use irc::proto::Command as Ic;
                    match c.send(Ic::Raw("SILENCE".to_string(), vec![format!("+{}", nick)])) {
                        Ok(()) => app.status_message = format!("Ignored {} (server-side).", nick),
                        Err(e) => app.status_message = format!("Ignore failed: {} (server may not support SILENCE)", e),
                    }
                } else {
                    app.status_message = "Not connected.".to_string();
                }
            }
        }
        R::Unignore(nick) => {
            let nick = nick.trim();
            if !nick.is_empty() {
                if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                    use irc::proto::Command as Ic;
                    match c.send(Ic::Raw("SILENCE".to_string(), vec![format!("-{}", nick)])) {
                        Ok(()) => app.status_message = format!("Unignored {}.", nick),
                        Err(e) => app.status_message = format!("Unignore failed: {}", e),
                    }
                } else {
                    app.status_message = "Not connected.".to_string();
                }
            }
        }
        R::NotificationsOn => {
            app.notifications_enabled = true;
            app.status_message = "Notifications on.".to_string();
        }
        R::NotificationsOff => {
            app.notifications_enabled = false;
            app.status_message = "Notifications off.".to_string();
        }
        R::Mute => {
            app.sounds_enabled = false;
            app.status_message = "Sounds muted.".to_string();
        }
        R::Unmute => {
            app.sounds_enabled = true;
            app.status_message = "Sounds unmuted.".to_string();
        }
        R::DebugTyping => {
            let n = app.typing_status.len();
            let preview: Vec<String> = app.typing_status.iter()
                .take(3)
                .map(|((_s, nick, t), (status, _))| format!("{}->{}:{}", nick, t, status))
                .collect();
            let mt = app.current_server.as_ref()
                .and_then(|srv| app.acked_caps_per_server.get(srv))
                .map(|s| s.contains("message-tags"))
                .unwrap_or(false);
            let mt_str = if n > 0 && !mt { "ok (inferred)" } else { if mt { "ok" } else { "false" } };
            app.status_message = format!(
                "Typing: {} entries {:?} | message-tags={}",
                n, preview, mt_str
            );
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
        R::Caps => {
            if let Some(server) = app.current_server.clone() {
                let requested = app.requested_caps_per_server.get(&server).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
                let acked_set = app.acked_caps_per_server.get(&server).cloned().unwrap_or_default();
                if requested.is_empty() {
                    app.status_message = "No capabilities negotiated yet.".to_string();
                } else {
                    for c in &requested {
                        let ok = acked_set.contains(c);
                        app.push_message(
                            &server,
                            "*server*",
                            MessageLine {
                                source: "***".to_string(),
                                text: format!("{} = {}", c, ok),
                                kind: MessageKind::Other,
                                image_id: None,
                                timestamp: None,
                                account: None,
                                msgid: None,
                                reply_to_msgid: None,
                            is_bot_sender: false,
                            },
                        );
                    }
                    app.status_message = format!("{} capability(ies)", requested.len());
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::Whois(nick) => {
            let target = if nick.is_empty() {
                app.current_channel.as_ref().and_then(|c| {
                    if c.starts_with('#') || c.starts_with('&') {
                        None
                    } else {
                        Some(c.clone())
                    }
                })
            } else {
                Some(nick)
            };
            match target {
                Some(n) if !n.is_empty() => {
                    if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                        let _ = c.send(IrcCommand::WHOIS(None, n.clone()));
                        app.whois_popup_visible = true;
                        app.whois_nick = n;
                        app.whois_lines = vec!["Requesting whois...".to_string()];
                    } else {
                        app.status_message = "Not connected.".to_string();
                    }
                }
                _ => {
                    app.status_message = "Usage: :whois <nick> or use in a DM window".to_string();
                }
            }
        }
        R::FocusUsers => {
            if app.user_panel_visible {
                app.panel_focus = PanelFocus::Users;
                request_channel_names(clients, app);
            }
        }
        R::SendPrivmsg { target, text } => {
            if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                for chunk in format::split_message_for_irc(&text, format::MAX_MESSAGE_BYTES) {
                    c.send_privmsg(&target, &chunk).map_err(|e| e.to_string())?;
                }
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
            } else if let (Some(server), Some((ref c, _))) = (app.current_server.clone(), app.current_server.as_ref().and_then(|s| clients.get(s.as_str()))) {
                let ephemeral = crypto::Keypair::generate();
                let ephemeral_pub_b64 = ephemeral.public_key_b64();
                let identity_pub_b64 = app.keypair.public_key_b64();
                let msg = format!("[:rvIRC:SECURE:INIT:{}:{}]", ephemeral_pub_b64, identity_pub_b64);
                c.send_privmsg(&nick, &msg).map_err(|e| e.to_string())?;
                app.pending_secure.insert(nick.clone());
                app.pending_secure_ephemeral.insert(nick.clone(), ephemeral);
                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                if !dms.contains(&nick) {
                    dms.push(nick.clone());
                }
                app.push_chat_log(&server, &nick, &format!("*** ESTABLISHING SECURE CONNECTION WITH {} ***", nick));
                app.push_chat_log(&server, &nick, "Sending key exchange request...");
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
            } else if let Some(server) = app.current_server.clone() {
                let sec_key = app::msg_key(&server, &nick);
                if app.secure_sessions.remove(&sec_key).is_some() {
                    app.push_chat_log(&server, &nick, "*** SECURE SESSION ENDED ***");
                    app.status_message = format!("Secure session with {} ended.", nick);
                } else {
                    app.status_message = format!("No secure session with {}.", nick);
                }
            } else {
                app.status_message = "Not connected.".to_string();
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
            } else if app.current_server.as_ref().and_then(|s| clients.get(s)).is_none() {
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
                } else if let Some(server) = app.current_server.clone() {
                        let tx = irc_tx.clone();
                    let nick_clone = nick.clone();
                    let server_clone = server.clone();
                    let file_name = file_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                        app.push_chat_log(&server, &nick, &format!("Starting file send: {}", file_name));
                    app.status_message = format!("Starting file send of {} to {}...", file_name, nick);
                    rt.spawn(async move {
                        match filetransfer::send_file(&file_path, server_clone.clone(), nick_clone.clone(), tx.clone()).await {
                            Ok(()) => {}
                            Err(e) => {
                                let _ = tx.send(IrcMessage::ChatLog {
                                    server: server_clone,
                                    target: nick_clone.clone(),
                                    text: format!("File send failed: {}", e),
                                });
                                let _ = tx.send(IrcMessage::Status(format!(
                                    "File send failed: {}", e
                                )));
                            }
                        }
                    });
                } else {
                    app.status_message = "Not connected.".to_string();
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
            } else if let Some(server) = app.current_server.clone() {
                let sec_key = app::msg_key(&server, &nick);
                if let Some(session) = app.secure_sessions.get(&sec_key) {
                    let words = session.sas_words();
                    let code = words.join(" ");
                    app.push_chat_log(&server, &nick, &format!("*** Verification code with {}: {} ***", nick, code));
                    app.push_chat_log(&server, &nick, "Both sides must run :verify -- ask your peer to run it too.");
                    app.push_chat_log(&server, &nick, "Compare the 6 words out-of-band (voice, in person, etc). If they match, run :verified");
                    app.status_message = format!("SAS: {}", code);
                } else {
                    app.status_message = format!("No secure session with {}.", nick);
                }
            } else {
                app.status_message = "Not connected.".to_string();
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
            } else if let Some(server) = app.current_server.clone() {
                if app.known_keys.set_verified(&nick, &server) {
                    if let Some(ref path) = app.known_keys_path {
                        let _ = app.known_keys.save(path);
                    }
                    app.push_chat_log(&server, &nick, &format!("*** {} is now marked as VERIFIED ***", nick));
                    app.status_message = format!("{} marked as verified.", nick);
                } else {
                    app.status_message = format!("No known key for {}.", nick);
                }
            } else {
                app.status_message = "Not connected.".to_string();
            }
        }
        R::UserAction { .. } => {}
    }
    Ok(false)
}

/// Tab completion: command name only (first word after :).
fn complete_input(app: &mut App) {
    const COMMANDS: &[&str] = &[
        "join", "part", "list", "superlist", "servers", "connect", "reconnect", "disconnect", "quit", "q", "clear", "invite", "away", "unban", "search",
        "msg", "me", "nick", "topic", "kick", "ban", "channel", "chan", "c",
        "channel-panel", "messages-panel", "user-panel", "friends-panel", "channels", "users",
        "version", "credits", "license", "caps",
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
fn request_channel_names(clients: &mut HashMap<String, (Client, tokio::task::JoinHandle<()>)>, app: &App) {
    if let (Some(ref server), Some(ref ch)) = (app.current_server.as_ref(), app.current_channel.as_ref()) {
        if ch.starts_with('#') || ch.starts_with('&') {
            if let Some((ref c, _)) = clients.get(server.as_str()) {
                let _ = c.send(IrcCommand::NAMES(Some(ch.to_string()), None));
            }
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
/// reply_to_msgid: when replying (r + number), the msgid we're replying to (shows ↷).
fn push_self_message(
    app: &mut App,
    server: &str,
    target: &str,
    text: String,
    reply_to_msgid: Option<String>,
    irc_tx: &IrcMessageTx,
    rt: &tokio::runtime::Runtime,
) {
    let nick = app.current_nickname.clone().unwrap_or_else(|| "?".to_string());
    let mut line = MessageLine { source: nick, text, kind: MessageKind::Privmsg, image_id: None, timestamp: None, account: None, msgid: None, reply_to_msgid, is_bot_sender: false };
    if app.render_images {
        if let Some(url) = extract_image_url(&line.text) {
            line.image_id = Some(app.next_image_id);
            app.next_image_id += 1;
            spawn_image_download(url, line.image_id.unwrap(), irc_tx, rt);
        }
    }
    app.push_message(server, target, line);
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

/// Return true if the string is a safe http(s) URL for image fetch: no control chars, and host is
/// not a private/local IP (SSRF mitigation).
fn is_safe_image_url(s: &str) -> bool {
    if !s.starts_with("http://") && !s.starts_with("https://") {
        return false;
    }
    if s.contains(|c: char| c == '\0' || c == '\n' || c == '\r' || c.is_control()) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(s) else { return false };
    let Some(host) = parsed.host_str() else { return false };
    is_public_image_host(host)
}

/// Reject private, loopback, and link-local IPs to prevent SSRF and privacy leaks.
fn is_public_image_host(host: &str) -> bool {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return !is_private_or_local_ip(ip);
    }
    true
}

fn is_private_or_local_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 127
                || o[0] == 10
                || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
        }
        std::net::IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0
                && s[6] == 0 && s[7] == 1
                || (s[0] & 0xffc0) == 0xfe80
        }
    }
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

/// Process queued protocol events (called from main loop, has access to clients).
fn process_protocol_events(
    app: &mut App,
    clients: &HashMap<String, (Client, tokio::task::JoinHandle<()>)>,
    rt: &tokio::runtime::Runtime,
    irc_tx: &IrcMessageTx,
) {
    use app::ProtocolEvent;
    let events: Vec<ProtocolEvent> = app.protocol_events.drain(..).collect();
    for evt in events {
        match evt {
            ProtocolEvent::SecureInit { from_nick, ephemeral_pub_b64, identity_pub_b64 } => {
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                if !dms.contains(&from_nick) {
                    dms.push(from_nick.clone());
                }
                app.push_chat_log(&server, &from_nick, &format!("*** {} REQUESTED SECURE CONNECTION ***", from_nick));

                // TOFU check
                let tofu = app.known_keys.check(&from_nick, &server, &identity_pub_b64);
                let key_changed = matches!(tofu, TofuResult::KeyChanged);

                match &tofu {
                    TofuResult::FirstContact => {
                        let fp = key_fingerprint(&identity_pub_b64);
                        app.push_chat_log(&server, &from_nick, &format!("First contact with {} -- key fingerprint: {}", from_nick, fp));
                    }
                    TofuResult::KeyMatch { verified } => {
                        if *verified {
                            app.push_chat_log(&server, &from_nick, &format!("Key matches known VERIFIED identity for {}.", from_nick));
                        } else {
                            app.push_chat_log(&server, &from_nick, &format!("Key matches known identity for {}.", from_nick));
                        }
                    }
                    TofuResult::KeyChanged => {
                        app.push_chat_log(&server, &from_nick, &format!("⚠ WARNING: {}'s identity key has CHANGED since last session!", from_nick));
                        app.push_chat_log(&server, &from_nick, "This could indicate a man-in-the-middle attack.");
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
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                if app.pending_secure.remove(&from_nick) {
                    app.push_chat_log(&server, &from_nick, "Key exchange response received...");

                    let their_identity_bytes: [u8; 32] = match base64::engine::general_purpose::STANDARD
                        .decode(&identity_pub_b64)
                        .ok()
                        .and_then(|v| v.try_into().ok())
                    {
                        Some(b) => b,
                        None => {
                            app.push_chat_log(&server, &from_nick, "Secure handshake failed: invalid identity key.");
                            continue;
                        }
                    };

                    // TOFU check
                    let tofu = app.known_keys.check(&from_nick, &server, &identity_pub_b64);
                    match &tofu {
                        TofuResult::FirstContact => {
                            let fp = key_fingerprint(&identity_pub_b64);
                            app.push_chat_log(&server, &from_nick, &format!("First contact with {} -- key fingerprint: {}", from_nick, fp));
                        }
                        TofuResult::KeyMatch { verified } => {
                            if *verified {
                                app.push_chat_log(&server, &from_nick, &format!("Key matches known VERIFIED identity for {}.", from_nick));
                            } else {
                                app.push_chat_log(&server, &from_nick, &format!("Key matches known identity for {}.", from_nick));
                            }
                        }
                        TofuResult::KeyChanged => {
                            app.push_chat_log(&server, &from_nick, &format!("⚠ WARNING: {}'s identity key has CHANGED since last session!", from_nick));
                            app.push_chat_log(&server, &from_nick, "This could indicate a man-in-the-middle attack. Session aborted.");
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
                            app.push_chat_log(&server, &from_nick, "Secure handshake failed: no ephemeral key found.");
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
                            app.secure_sessions.insert(app::msg_key(&server, &from_nick), session);
                            let fp = key_fingerprint(&identity_pub_b64);
                            app.push_chat_log(&server, &from_nick, &format!("Key fingerprint: {}", fp));
                            app.push_chat_log(&server, &from_nick, "*** SECURE CONNECTION ESTABLISHED ***");
                            app.push_chat_log(&server, &from_nick, "Messages are now end-to-end encrypted (X25519 + ChaCha20-Poly1305).");
                            if !app.known_keys.is_verified(&from_nick, &server) {
                                app.push_chat_log(&server, &from_nick, "Use :verify to compare verification codes.");
                            }
                            app.status_message = format!("Secure session established with {}.", from_nick);
                        }
                        Err(e) => {
                            app.push_chat_log(&server, &from_nick, &format!("Secure handshake failed: {}", e));
                            app.status_message = format!("Secure handshake with {} failed: {}", from_nick, e);
                        }
                    }
                }
            }
            ProtocolEvent::Encrypted { from_nick, nonce_b64, ciphertext_b64 } => {
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                let sec_key = app::msg_key(&server, &from_nick);
                if let Some(session) = app.secure_sessions.get_mut(&sec_key) {
                    match session.decrypt(&nonce_b64, &ciphertext_b64) {
                        Ok(plaintext) => {
                            let mut line = MessageLine {
                                source: from_nick.clone(),
                                text: plaintext,
                                kind: MessageKind::Privmsg,
                                image_id: None,
                                timestamp: None,
                                account: None,
                                msgid: None,
                                reply_to_msgid: None,
                            is_bot_sender: false,
                            };
                            if app.render_images {
                                if let Some(url) = extract_image_url(&line.text) {
                                    let image_id = app.next_image_id;
                                    app.next_image_id += 1;
                                    line.image_id = Some(image_id);
                                    spawn_image_download(url, image_id, irc_tx, rt);
                                }
                            }
                            app.push_message(&server, &from_nick, line);
                        }
                        Err(e) => {
                            app.push_message(
                                &server,
                                &from_nick,
                                MessageLine {
                                    source: "rvIRC".to_string(),
                                    text: format!("[decrypt error: {}]", e),
                                    kind: MessageKind::Other,
                                    image_id: None,
                                    timestamp: None,
                                    account: None,
                                    msgid: None,
                                    reply_to_msgid: None,
                            is_bot_sender: false,
                                },
                            );
                        }
                    }
                } else {
                    // No session: if we know this peer (TOFU), auto-initiate re-secure once per 60s
                    let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                    let known = app.known_keys.lookup(&from_nick, &server).is_some();
                    let already_pending = app.pending_secure.contains(&from_nick);
                    let rate_ok = app.last_auto_rekey.get(&from_nick).map_or(true, |t| {
                        std::time::Instant::now().duration_since(*t) >= std::time::Duration::from_secs(60)
                    });

                    if known && !already_pending && rate_ok {
                        app.last_auto_rekey.insert(from_nick.clone(), std::time::Instant::now());
                        if let Some((ref c, _)) = app.current_server.as_ref().and_then(|s| clients.get(s)) {
                            let ephemeral = crypto::Keypair::generate();
                            let ephemeral_pub_b64 = ephemeral.public_key_b64();
                            let identity_pub_b64 = app.keypair.public_key_b64();
                            let msg = format!("[:rvIRC:SECURE:INIT:{}:{}]", ephemeral_pub_b64, identity_pub_b64);
                            if c.send_privmsg(&from_nick, &msg).is_ok() {
                                app.pending_secure.insert(from_nick.clone());
                                app.pending_secure_ephemeral.insert(from_nick.clone(), ephemeral);
                                let dms = app.dm_targets_per_server.entry(server.to_string()).or_default();
                                if !dms.contains(&from_nick) {
                                    dms.push(from_nick.clone());
                                }
                                app.push_chat_log(&server, &from_nick, "*** No secure session. Sending key exchange to re-establish... ***");
                            }
                        }
                    }

                    app.push_message(
                        &server,
                        &from_nick,
                        MessageLine {
                            source: from_nick.clone(),
                            text: "[encrypted message, no secure session]".to_string(),
                            kind: MessageKind::Other,
                            image_id: None,
                            timestamp: None,
                            account: None,
                            msgid: None,
                            reply_to_msgid: None,
                            is_bot_sender: false,
                        },
                    );
                }
            }
            ProtocolEvent::WormholeOffer { from_nick, code, filename, size } => {
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                let size_display = if size >= 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size >= 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} B", size)
                };
                app.push_chat_log(&server, &from_nick, &format!(
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
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                app.push_chat_log(&server, &from_nick, "*** File transfer completed. ***");
                app.status_message = format!("File transfer to {} completed.", from_nick);
            }
            ProtocolEvent::WormholeReject { from_nick } => {
                let server = app.current_server.as_deref().unwrap_or("unknown").to_string();
                app.push_chat_log(&server, &from_nick, "*** File transfer rejected. ***");
                app.status_message = format!("{} rejected the file transfer.", from_nick);
            }
        }
    }
}
