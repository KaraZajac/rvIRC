//! App state: mode, channels, users, message buffers, input, pane visibility.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::crypto::{Keypair, KnownKeys, SecureSession};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileBrowserMode {
    /// Choosing a directory to save a received file into.
    ReceiveFile,
    /// Choosing a file to send.
    SendFile,
}

/// Protocol events from intercepted [:rvIRC:] messages, queued for main loop processing.
#[derive(Debug)]
pub enum ProtocolEvent {
    SecureInit { from_nick: String, ephemeral_pub_b64: String, identity_pub_b64: String },
    SecureAck { from_nick: String, ephemeral_pub_b64: String, identity_pub_b64: String },
    Encrypted { from_nick: String, nonce_b64: String, ciphertext_b64: String },
    WormholeOffer { from_nick: String, code: String, filename: String, size: u64 },
    WormholeComplete { from_nick: String },
    WormholeReject { from_nick: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
}

#[derive(Debug, Clone)]
pub struct MessageLine {
    pub source: String,
    pub text: String,
    pub kind: MessageKind,
    pub image_id: Option<usize>,
    /// When the message was received (for display as nick | HH:mm).
    pub timestamp: Option<chrono::DateTime<chrono::Local>>,
    /// IRCv3 account-tag: account name when sender is logged in (None = not logged in or unknown).
    pub account: Option<String>,
    /// IRCv3 message-ids: server-provided unique ID for this message (enables reply threading).
    #[allow(dead_code)] // Stored for future reply-send and threading UI
    pub msgid: Option<String>,
    /// IRCv3 reply tag: msgid of the message this is replying to (client-only +reply tag).
    pub reply_to_msgid: Option<String>,
    /// bot-mode: sender has set bot mode (from message tags).
    pub is_bot_sender: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Privmsg,
    Notice,
    Action, // /me
    Join,
    Part,
    Quit,
    Nick,
    Mode,
    Other,
}

#[derive(Debug, Clone)]
pub enum UserAction {
    Dm,
    Kick,
    Ban,
    Unban,
    Op,
    Deop,
    Voice,
    Devoice,
    Halfop,
    Dehalfop,
    Mute,
    Whois,
}

/// Which panel has focus in Normal mode (keyboard input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Main,
    Channels,
    Messages,
    Users,
    Friends,
}

pub struct App {
    pub mode: Mode,
    pub input: String,
    pub input_cursor: usize,
    /// Selection in input: (start, end) byte indices, start <= end. None means no selection.
    pub input_selection: Option<(usize, usize)>,

    /// IRCv3 caps acked per server (server -> set of cap names).
    pub acked_caps_per_server: HashMap<String, std::collections::HashSet<String>>,
    /// Caps we requested per server (server -> list of cap names, for :caps display).
    pub requested_caps_per_server: HashMap<String, Vec<String>>,
    /// When the app was created; used for rainbow animation phase.
    pub created_at: Instant,

    pub channel_panel_visible: bool,
    pub messages_panel_visible: bool,
    /// Connected servers in display order.
    pub connected_servers: Vec<String>,
    /// Channels per server (server -> channels).
    pub channels_per_server: HashMap<String, Vec<String>>,
    pub channel_index: usize,
    /// Selection index within Messages (DM) list.
    pub messages_index: usize,

    pub user_panel_visible: bool,
    pub friends_panel_visible: bool,
    pub user_list: Vec<String>,
    pub user_index: usize,
    /// Users pane: filter text (active when focused); f to focus, type to filter, Esc to unfocus.
    pub user_list_filter: String,
    /// When true, key events add to user_list_filter instead of navigating.
    pub user_list_filter_focused: bool,
    pub user_action_menu: bool,
    pub user_action_index: usize,
    /// Friends list per server (MONITOR targets). Persisted in friends.toml.
    pub friends_per_server: HashMap<String, Vec<String>>,
    /// Friends currently online per server (from RPL_MONONLINE 730).
    pub friends_online: HashMap<String, HashSet<String>>,
    /// Friends currently away per server (from away-notify AWAY).
    pub friends_away: HashMap<String, HashSet<String>>,
    /// Selection index within friends list.
    pub friends_index: usize,
    /// Path to friends.toml for saving.
    pub friends_path: Option<PathBuf>,

    pub panel_focus: PanelFocus,

    pub current_channel: Option<String>,
    pub current_server: Option<String>,
    pub current_nickname: Option<String>,
    /// DM targets per server (server -> nicks).
    pub dm_targets_per_server: HashMap<String, Vec<String>>,
    pub messages: HashMap<String, Vec<MessageLine>>,
    /// draft/react: msgid -> [(nick, emoji)] for reactions displayed on messages.
    pub reactions: HashMap<String, Vec<(String, String)>>,

    /// :search popup: filter, results (index, preview), selection.
    pub search_popup_visible: bool,
    pub search_filter: String,
    pub search_results: Vec<(usize, String)>,
    pub search_selected_index: usize,
    /// false = filter mode (type to search), true = scroll mode (j/k, Enter jumps).
    pub search_scroll_mode: bool,

    /// :list popup: server channel list (from LIST command), filter, selection.
    /// Each entry is (server, channel name, optional user count).
    pub channel_list_popup_visible: bool,
    pub server_channel_list: Vec<(String, String, Option<u32>)>,
    pub channel_list_filter: String,
    pub channel_list_selected_index: usize,
    /// false = filter mode (type to search), true = scroll mode (j/k/arrows move, Enter joins).
    pub channel_list_scroll_mode: bool,
    /// For :list: server we listed (used when joining). For :superlist: None (server in each entry).
    pub channel_list_server: Option<String>,
    /// true = :superlist mode (append ChannelList from each server). false = :list (replace).
    pub channel_list_super: bool,
    /// For :superlist: servers we're still waiting for LIST response.
    pub channel_list_pending_servers: std::collections::HashSet<String>,

    /// :servers popup: list of server names from config, select one to connect.
    pub server_list_popup_visible: bool,
    pub server_list: Vec<String>,
    pub server_list_selected_index: usize,

    /// Whois popup: show result when user selects Whois from user action menu.
    pub whois_popup_visible: bool,
    pub whois_nick: String,
    pub whois_lines: Vec<String>,

    /// :credits popup.
    pub credits_popup_visible: bool,
    /// :license popup.
    pub license_popup_visible: bool,
    /// License popup vertical scroll (lines).
    pub license_popup_scroll_offset: usize,

    /// Message area scroll: 0 = show latest; increase when user scrolls up (see older).
    pub message_scroll_offset: usize,
    /// When set, next sent PRIVMSG will include +reply tag with this msgid (IRCv3 reply).
    pub reply_to_msgid: Option<String>,
    /// Reply-select mode: r pressed, waiting for 1-9 or 0 to pick which message to reply to.
    pub reply_select_mode: bool,
    /// Recently sent (server, target, text) for deduplication with ZNC/bouncer echo (avoids duplicate when client nick differs).
    pub recent_sents: Vec<(String, String, String, std::time::Instant)>,
    /// (server, target) for which we've sent CHATHISTORY BEFORE and are waiting for batch.
    pub chathistory_before_pending: Option<(String, String)>,

    /// Servers that need auto_join run (Connected received, channels from config).
    pub pending_auto_join_servers: HashSet<String>,
    /// Per-server: when to run auto_join (identify first, then join). Entry present = delay until that time.
    pub auto_join_after_per_server: HashMap<String, Instant>,

    /// Channel topic (per target). Empty = no topic.
    pub channel_topics: HashMap<String, String>,
    /// Channel modes (per channel). e.g. "+nt"
    pub channel_modes: HashMap<String, String>,
    /// Last invite: "nick invited you to #channel" for status/join.
    pub last_invite: Option<(String, String)>,
    /// account-notify/account-tag/extended-join: (server, nick) -> account (None = logged out).
    pub account_per_nick: HashMap<(String, String), Option<String>>,
    /// userhost-in-names: (server, nick) -> user@host for user list display.
    pub userhost_per_nick: HashMap<(String, String), String>,
    /// bot-mode: (server, nick) -> true if we've seen a message with bot tag from this nick.
    pub bot_per_nick: std::collections::HashSet<(String, String)>,
    /// Muted nicks per channel. Key = "server/target" or "server/*" for global, value = set of nicks.
    pub muted_nicks: HashMap<String, std::collections::HashSet<String>>,

    /// Input history (newest first). Capped at 100.
    pub input_history: Vec<String>,
    pub input_history_index: usize, // 0 = not browsing; when > 0 we show history[index - 1]
    pub input_draft: String,        // saved when browsing history, restored when Down to 0

    /// Targets with unread messages. Key = "server/target".
    pub unread_targets: HashSet<String>,
    /// Targets with an unread mention. Key = "server/target".
    pub unread_mentions: HashSet<String>,

    /// Auto-reconnect after disconnect: server name, when to try next, attempt (1..=3). Delays: 5s, 15s, 30s.
    pub reconnect_server: Option<String>,
    pub reconnect_after: Option<Instant>,
    pub reconnect_attempt: u8,

    /// Persistent identity keypair (loaded from ~/.config/rvIRC/identity.toml).
    pub keypair: Keypair,
    /// Active encrypted sessions keyed by nick (case-sensitive as received from IRC).
    pub secure_sessions: HashMap<String, SecureSession>,
    /// Nicks where we sent a SECURE:INIT and are waiting for ACK.
    pub pending_secure: HashSet<String>,
    /// Last auto-rekey time per nick (rate limit: 60s).
    pub last_auto_rekey: std::collections::HashMap<String, std::time::Instant>,
    /// Ephemeral keypairs generated per :secure initiation, keyed by nick.
    pub pending_secure_ephemeral: HashMap<String, Keypair>,
    /// TOFU known keys store.
    pub known_keys: KnownKeys,
    /// Path to known_keys.toml for saving.
    pub known_keys_path: Option<PathBuf>,
    /// Queued protocol events from intercepted [:rvIRC:] messages.
    pub protocol_events: Vec<ProtocolEvent>,

    /// Secure accept popup (incoming SECURE:INIT that needs user confirmation).
    pub secure_accept_popup_visible: bool,
    pub secure_accept_nick: String,
    pub secure_accept_ephemeral_b64: String,
    pub secure_accept_identity_b64: String,
    pub secure_accept_key_changed: bool,

    /// File receive offer popup.
    pub file_receive_popup_visible: bool,
    pub file_receive_nick: String,
    pub file_receive_filename: String,
    pub file_receive_size: u64,
    pub file_receive_code: String,

    /// Wormhole transfer progress popup (shown during send/receive).
    pub transfer_progress_visible: bool,
    pub transfer_progress_nick: String,
    pub transfer_progress_filename: String,
    pub transfer_progress_bytes: u64,
    pub transfer_progress_total: u64,
    pub transfer_progress_is_send: bool,

    /// File browser popup (for choosing save directory).
    pub file_browser_visible: bool,
    pub file_browser_path: PathBuf,
    pub file_browser_entries: Vec<(String, bool)>,
    pub file_browser_selected_index: usize,
    /// What the file browser is being used for.
    pub file_browser_mode: FileBrowserMode,
    /// Pending filename to save after directory is chosen.
    pub file_browser_pending_filename: String,
    /// Pending wormhole code to use after directory is chosen.
    pub file_browser_pending_code: String,
    /// Pending nick who sent the file.
    pub file_browser_pending_nick: String,

    pub status_message: String,
    /// Current away message (None = not away).
    pub away_message: Option<String>,
    /// Yellow away popup visible; any key dismisses and clears away.
    pub away_popup_visible: bool,

    /// Whether to fetch and render inline images for image URLs (from config).
    pub render_images: bool,
    /// Whether to show offline friends: None/"show" = show (red), Some("hide") = hide.
    pub offline_friends: Option<String>,
    /// Show desktop notifications for messages in other buffers.
    pub notifications_enabled: bool,
    /// Play sound with notifications (:mute / :unmute toggle).
    pub sounds_enabled: bool,
    pub next_image_id: usize,
    pub inline_images: HashMap<usize, InlineImage>,

    /// :highlight popup: list of words to highlight in messages.
    pub highlight_popup_visible: bool,
    pub highlight_words: Vec<String>,
    pub highlight_input: String,
    pub highlight_selected_index: usize,
    /// Path to highlight.toml.
    pub highlight_path: Option<PathBuf>,
    /// Path to read_markers.toml.
    pub read_markers_path: Option<PathBuf>,

    /// IRCv3 typing: (server, nick, target) -> (status, received_at). Shown with 6s timeout for active, 30s for paused.
    pub typing_status: HashMap<(String, String, String), (String, Instant)>,
    /// When we last sent a typing indicator per target (for throttle: max once per 3s).
    pub last_typing_sent: HashMap<String, Instant>,

    /// Servers that advertised UTF8ONLY in their ISUPPORT (005) burst.
    pub utf8only_servers: HashSet<String>,
    /// draft/extended-isupport: NETWORK= name per server from RPL_ISUPPORT (005).
    pub isupport_network_name: HashMap<String, String>,
    /// draft/network-icon: NETWORKICON= URL per server from RPL_ISUPPORT (005).
    pub isupport_network_icon: HashMap<String, String>,
    /// draft/metadata: (server, target) -> key -> value. Populated from server-pushed METADATA events and RPL_KEYVALUE (761).
    pub user_metadata: HashMap<(String, String), HashMap<String, String>>,
    /// WHOX query token counter (1–999, wraps). Used to correlate WHO requests with responses.
    pub whox_token: u16,

    /// Ban list popup state.
    pub ban_popup_visible: bool,
    pub ban_popup_channel: String,
    pub ban_popup_entries: Vec<String>,
    /// Scroll offset for ban list popup.
    pub ban_popup_scroll: usize,

    /// Edit select mode: e pressed, waiting for 1-9 or 0 to pick which message to edit.
    pub edit_select_mode: bool,
    /// Edit popup: visible when a message is selected for editing.
    pub edit_popup_visible: bool,
    /// msgid of the message being edited.
    pub edit_popup_msgid: String,
    /// Current text in the edit popup input field.
    pub edit_popup_input: String,
    /// Cursor position (byte offset) in edit_popup_input.
    pub edit_popup_cursor: usize,
}

/// An inline image: either a static frame or an animated GIF with pre-encoded frames.
pub enum InlineImage {
    Static(ratatui_image::protocol::StatefulProtocol),
    Animated {
        frames: Vec<ratatui_image::protocol::StatefulProtocol>,
        delays: Vec<Duration>,
        current_frame: usize,
        last_advance: Instant,
    },
}

impl InlineImage {
    pub fn protocol_mut(&mut self) -> &mut ratatui_image::protocol::StatefulProtocol {
        match self {
            InlineImage::Static(p) => p,
            InlineImage::Animated { frames, current_frame, .. } => &mut frames[*current_frame],
        }
    }

    /// Advance to the next frame if enough time has elapsed. Only call for visible images.
    pub fn advance_frame(&mut self) {
        if let InlineImage::Animated { frames, delays, current_frame, last_advance } = self {
            if frames.is_empty() { return; }
            let delay = delays.get(*current_frame).copied()
                .unwrap_or(Duration::from_millis(100));
            if last_advance.elapsed() >= delay {
                *current_frame = (*current_frame + 1) % frames.len();
                *last_advance = Instant::now();
            }
        }
    }
}

/// Composite key for server+target (messages, unread, etc.).
/// Channel names (# and &) are normalized to lowercase per IRC RFC (case-insensitive).
pub fn msg_key(server: &str, target: &str) -> String {
    let t = if (target.starts_with('#') || target.starts_with('&')) && !target.is_empty() {
        target.to_lowercase()
    } else {
        target.to_string()
    };
    format!("{}/{}", server, t)
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            input_selection: None,
            channel_panel_visible: true,
            messages_panel_visible: true,
            connected_servers: Vec::new(),
            channels_per_server: HashMap::new(),
            channel_index: 0,
            messages_index: 0,
            user_panel_visible: true,
            friends_panel_visible: true,
            user_list: Vec::new(),
            user_index: 0,
            user_list_filter: String::new(),
            user_list_filter_focused: false,
            user_action_menu: false,
            user_action_index: 0,
            friends_per_server: HashMap::new(),
            friends_online: HashMap::new(),
            friends_away: HashMap::new(),
            friends_index: 0,
            friends_path: None,
            panel_focus: PanelFocus::Main,
            current_channel: None,
            current_server: None,
            current_nickname: None,
            dm_targets_per_server: HashMap::new(),
            messages: HashMap::new(),
            reactions: HashMap::new(),
            search_popup_visible: false,
            search_filter: String::new(),
            search_results: Vec::new(),
            search_selected_index: 0,
            search_scroll_mode: false,
            channel_list_popup_visible: false,
            server_channel_list: Vec::new(),
            channel_list_filter: String::new(),
            channel_list_selected_index: 0,
            channel_list_scroll_mode: false,
            channel_list_server: None,
            channel_list_super: false,
            channel_list_pending_servers: std::collections::HashSet::new(),
            server_list_popup_visible: false,
            server_list: Vec::new(),
            server_list_selected_index: 0,
            whois_popup_visible: false,
            whois_nick: String::new(),
            whois_lines: Vec::new(),
            credits_popup_visible: false,
            license_popup_visible: false,
            license_popup_scroll_offset: 0,
            message_scroll_offset: 0,
            reply_to_msgid: None,
            reply_select_mode: false,
            recent_sents: Vec::new(),
            chathistory_before_pending: None,
            acked_caps_per_server: HashMap::new(),
            requested_caps_per_server: HashMap::new(),
            created_at: Instant::now(),
            pending_auto_join_servers: HashSet::new(),
            auto_join_after_per_server: HashMap::new(),
            channel_topics: HashMap::new(),
            channel_modes: HashMap::new(),
            last_invite: None,
            account_per_nick: HashMap::new(),
            userhost_per_nick: HashMap::new(),
            bot_per_nick: std::collections::HashSet::new(),
            muted_nicks: HashMap::new(),
            input_history: Vec::new(),
            input_history_index: 0,
            input_draft: String::new(),
            unread_targets: HashSet::new(),
            unread_mentions: HashSet::new(),
            reconnect_server: None,
            reconnect_after: None,
            reconnect_attempt: 0,
            keypair: Keypair::generate(),
            secure_sessions: HashMap::new(),
            pending_secure: HashSet::new(),
            last_auto_rekey: HashMap::new(),
            pending_secure_ephemeral: HashMap::new(),
            known_keys: KnownKeys::default(),
            known_keys_path: None,
            protocol_events: Vec::new(),
            secure_accept_popup_visible: false,
            secure_accept_nick: String::new(),
            secure_accept_ephemeral_b64: String::new(),
            secure_accept_identity_b64: String::new(),
            secure_accept_key_changed: false,
            file_receive_popup_visible: false,
            file_receive_nick: String::new(),
            file_receive_filename: String::new(),
            file_receive_size: 0,
            file_receive_code: String::new(),
            transfer_progress_visible: false,
            transfer_progress_nick: String::new(),
            transfer_progress_filename: String::new(),
            transfer_progress_bytes: 0,
            transfer_progress_total: 0,
            transfer_progress_is_send: false,
            file_browser_visible: false,
            file_browser_path: PathBuf::new(),
            file_browser_entries: Vec::new(),
            file_browser_selected_index: 0,
            file_browser_mode: FileBrowserMode::ReceiveFile,
            file_browser_pending_filename: String::new(),
            file_browser_pending_code: String::new(),
            file_browser_pending_nick: String::new(),
            status_message: String::new(),
            away_message: None,
            away_popup_visible: false,

            render_images: true,
            offline_friends: None,
            notifications_enabled: true,
            sounds_enabled: true,
            next_image_id: 0,
            inline_images: HashMap::new(),
            highlight_popup_visible: false,
            highlight_words: Vec::new(),
            highlight_input: String::new(),
            highlight_selected_index: 0,
            highlight_path: None,
            read_markers_path: None,
            typing_status: HashMap::new(),
            last_typing_sent: HashMap::new(),
            utf8only_servers: HashSet::new(),
            isupport_network_name: HashMap::new(),
            isupport_network_icon: HashMap::new(),
            user_metadata: HashMap::new(),
            whox_token: 1,
            ban_popup_visible: false,
            ban_popup_channel: String::new(),
            ban_popup_entries: Vec::new(),
            ban_popup_scroll: 0,
            edit_select_mode: false,
            edit_popup_visible: false,
            edit_popup_msgid: String::new(),
            edit_popup_input: String::new(),
            edit_popup_cursor: 0,
        }
    }

    #[allow(dead_code)]
    pub fn has_secure_session(&self, nick: &str) -> bool {
        self.secure_sessions.contains_key(nick)
    }

    /// Populate file_browser_entries from the directory at file_browser_path.
    pub fn refresh_file_browser(&mut self) {
        let mut entries = Vec::new();
        if let Ok(read_dir) = std::fs::read_dir(&self.file_browser_path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push((name, is_dir));
            }
        }
        entries.sort_by(|a, b| {
            b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        self.file_browser_entries = entries;
        self.file_browser_selected_index = 0;
    }

    /// Clear auto-reconnect state (e.g. after manual connect or quit).
    pub fn clear_reconnect(&mut self) {
        self.reconnect_server = None;
        self.reconnect_after = None;
        self.reconnect_attempt = 0;
    }

    /// Clear unread/mention state for a target when user switches to it.
    pub fn mark_target_read(&mut self, server: &str, target: &str) {
        let key = msg_key(server, target);
        self.unread_targets.remove(&key);
        self.unread_mentions.remove(&key);
    }

    /// Nicks currently typing in target (active: 6s timeout, paused: 30s). Excludes ourselves.
    pub fn typing_nicks_for_target(&self, server: &str, target: &str) -> Vec<String> {
        let now = Instant::now();
        let our_nick = self.current_nickname.as_deref().unwrap_or("");
        self.typing_status
            .iter()
            .filter(|((s, nick, t), (status, at))| {
                s == server
                    && (*t == target || (*t == our_nick && nick.as_str() == target))
                    && *nick != our_nick
                    && (status.as_str() == "active" && now.duration_since(*at).as_secs() < 6
                        || status.as_str() == "paused" && now.duration_since(*at).as_secs() < 30)
            })
            .map(|((_, nick, _), _)| nick.clone())
            .collect()
    }

    /// Filtered server channel list for the :list popup (substring match on channel or server, case-insensitive).
    pub fn filtered_server_channel_list(&self) -> Vec<(String, String, Option<u32>)> {
        let q = self.channel_list_filter.to_lowercase();
        if q.is_empty() {
            return self.server_channel_list.clone();
        }
        self.server_channel_list
            .iter()
            .filter(|(server, channel, _)| {
                server.to_lowercase().contains(&q) || channel.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    /// Clamp channel_list_selected_index to filtered list length.
    pub fn clamp_channel_list_selected_index(&mut self) {
        let len = self.filtered_server_channel_list().len();
        if len == 0 {
            self.channel_list_selected_index = 0;
        } else {
            self.channel_list_selected_index = self.channel_list_selected_index.min(len - 1);
        }
    }

    /// Selected (server, channel) in the list popup (from filtered list), for joining.
    pub fn selected_list_channel_and_server(&self) -> Option<(String, String)> {
        let filtered = self.filtered_server_channel_list();
        filtered
            .get(self.channel_list_selected_index)
            .map(|(server, channel, _)| (server.clone(), channel.clone()))
    }

    /// Clamp server_list_selected_index to server_list length.
    pub fn clamp_server_list_selected_index(&mut self) {
        let len = self.server_list.len();
        if len == 0 {
            self.server_list_selected_index = 0;
        } else {
            self.server_list_selected_index = self.server_list_selected_index.min(len - 1);
        }
    }

    /// Selected server name in the :servers popup, if any.
    pub fn selected_server_name(&self) -> Option<String> {
        self.server_list.get(self.server_list_selected_index).cloned()
    }

    /// Channels pane: flat list of (server, target) for selection. Server name row, then indented channels.
    pub fn channels_list(&self) -> Vec<(String, String)> {
        let mut list = Vec::new();
        for server in &self.connected_servers {
            list.push((server.clone(), "*server*".to_string()));
            if let Some(chans) = self.channels_per_server.get(server) {
                for ch in chans {
                    list.push((server.clone(), ch.clone()));
                }
            }
        }
        list
    }

    /// Messages pane: flat list of (server, nick) DMs, grouped by server.
    pub fn messages_list(&self) -> Vec<(String, String)> {
        let mut list = Vec::new();
        for server in &self.connected_servers {
            if let Some(nicks) = self.dm_targets_per_server.get(server) {
                for nick in nicks {
                    list.push((server.clone(), nick.clone()));
                }
            }
        }
        list
    }

    /// Selected (server, target) from Channels panel.
    pub fn selected_channel_entry(&self) -> Option<(String, String)> {
        let list = self.channels_list();
        let idx = self.channel_index.min(list.len().saturating_sub(1));
        list.get(idx).cloned()
    }

    /// Selected (server, nick) from Messages panel.
    pub fn selected_message_entry(&self) -> Option<(String, String)> {
        let list = self.messages_list();
        let idx = self.messages_index.min(list.len().saturating_sub(1));
        list.get(idx).cloned()
    }

    /// Selected target based on panel focus (target only, for backward compat).
    #[allow(dead_code)]
    pub fn selected_target(&self) -> Option<String> {
        match self.panel_focus {
            PanelFocus::Channels => self.selected_channel_entry().map(|(_, t)| t),
            PanelFocus::Messages => self.selected_message_entry().map(|(_, n)| n),
            _ => self.current_channel.clone(),
        }
    }

    pub fn current_messages(&self) -> &[MessageLine] {
        let server = self.current_server.as_deref().unwrap_or("");
        let target = self.current_channel.as_deref().unwrap_or("*server*");
        let key = msg_key(server, target);
        self.messages
            .get(&key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Replyable messages (with msgid, unmuted) in chronological order.
    /// Used for reply-select: 1 = last, 2 = second-to-last, etc.
    pub fn replyable_msgids(&self) -> Vec<String> {
        let target_key = msg_key(
            self.current_server.as_deref().unwrap_or(""),
            self.current_channel.as_deref().unwrap_or("*server*"),
        );
        self.current_messages()
            .iter()
            .filter(|m| m.msgid.is_some() && !self.is_muted(&target_key, &m.source))
            .filter_map(|m| m.msgid.clone())
            .collect()
    }

    /// msgid -> display number (1–9, 10 for "0") for reply-select mode. Only last 10 get numbers.
    pub fn reply_select_numbers(&self) -> std::collections::HashMap<String, u8> {
        let ids = self.replyable_msgids();
        let len = ids.len();
        let mut map = std::collections::HashMap::new();
        for (i, msgid) in ids.iter().enumerate().rev().take(10) {
            let num = (len - i) as u8;
            map.insert(msgid.clone(), if num == 10 { 10 } else { num });
        }
        map
    }

    /// Record a recently sent message for ZNC/bouncer deduplication (client nick may differ from echoed source).
    pub fn record_recent_send(&mut self, server: &str, target: &str, text: &str) {
        let now = std::time::Instant::now();
        self.recent_sents.push((server.to_string(), target.to_string(), text.to_string(), now));
        self.recent_sents.retain(|(_, _, _, t)| now.duration_since(*t).as_secs() < 5);
        if self.recent_sents.len() > 20 {
            self.recent_sents.drain(0..self.recent_sents.len() - 20);
        }
    }

    /// True if this received message matches a recent send (dedupe ZNC echo when client nick differs).
    pub fn is_recent_sent(&self, server: &str, effective_target: &str, text: &str) -> bool {
        let now = std::time::Instant::now();
        self.recent_sents.iter().any(|(s, t, m, ts)| {
            s == server && t == effective_target && m == text && now.duration_since(*ts).as_secs() < 5
        })
    }

    /// Update search_results from current_messages filtered by search_filter (case-insensitive).
    pub fn update_search_results(&mut self) {
        let server = self.current_server.as_deref().unwrap_or("");
        let target = self.current_channel.as_deref().unwrap_or("*server*");
        let target_key = msg_key(server, target);
        let messages = self.current_messages();
        let filter_lower = self.search_filter.to_lowercase();
        self.search_results = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !self.is_muted(&target_key, &m.source))
            .filter(|(_, m)| filter_lower.is_empty() || m.text.to_lowercase().contains(&filter_lower))
            .map(|(i, m)| {
                let preview = format!("{}: {}", m.source, m.text);
                let preview = preview.chars().take(60).collect::<String>();
                (i, preview)
            })
            .collect();
        self.search_selected_index = self.search_selected_index.min(self.search_results.len().saturating_sub(1));
    }

    /// Whether to hide messages from this nick in the given target (mute list).
    /// key = "server/target" or "server/*" for global mute on that server.
    pub fn is_muted(&self, key: &str, nick: &str) -> bool {
        self.muted_nicks
            .get(key)
            .map(|s| s.contains(nick))
            .unwrap_or(false)
            || key
                .split('/')
                .next()
                .map(|s| self.muted_nicks.get(&format!("{}/*", s)).map(|m| m.contains(nick)).unwrap_or(false))
                .unwrap_or(false)
    }

    pub fn current_topic(&self) -> Option<&str> {
        let server = self.current_server.as_deref()?;
        let target = self.current_channel.as_deref()?;
        if target.starts_with('#') || target.starts_with('&') {
            let key = msg_key(server, target);
            self.channel_topics.get(&key).map(|s| s.as_str()).filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    pub fn current_modes(&self) -> Option<&str> {
        let server = self.current_server.as_deref()?;
        let target = self.current_channel.as_deref()?;
        if target.starts_with('#') || target.starts_with('&') {
            let key = msg_key(server, target);
            self.channel_modes.get(&key).map(|s| s.as_str()).filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    /// Push a system/status message into a DM/channel chat window.
    pub fn push_chat_log(&mut self, server: &str, target: &str, text: &str) {
        self.push_message(
            server,
            target,
            MessageLine {
                source: "***".to_string(),
                text: text.to_string(),
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

    /// If the current channel is a DM (not a #channel or *server*), return the nick.
    pub fn current_dm_nick(&self) -> Option<String> {
        self.current_channel.as_ref().and_then(|ch| {
            if ch.starts_with('#') || ch.starts_with('&') || ch == "*server*" {
                None
            } else {
                Some(ch.clone())
            }
        })
    }

    pub fn push_message(&mut self, server: &str, target: &str, mut line: MessageLine) {
        let key = msg_key(server, target);
        let current_key = self
            .current_server
            .as_deref()
            .and_then(|s| self.current_channel.as_deref().map(|t| msg_key(s, t)))
            .unwrap_or_default();
        let mention = self
            .current_nickname
            .as_deref()
            .map_or(false, |nick| !nick.is_empty() && line.text.to_lowercase().contains(&nick.to_lowercase()));
        let highlight = self.highlight_matches(&line.text);
        if line.timestamp.is_none() {
            line.timestamp = Some(chrono::Local::now());
        }
        self.messages
            .entry(key.clone())
            .or_default()
            .push(line);
        if key != current_key {
            self.unread_targets.insert(key.clone());
            if mention || highlight {
                self.unread_mentions.insert(key);
            }
        }
    }

    /// Save current target's read position (scroll offset) to read_markers.
    pub fn save_current_read_marker(&self) {
        let Some(ref target) = self.current_channel else { return };
        if target.is_empty() {
            return;
        }
        if let Some(ref path) = self.read_markers_path {
            let _ = crate::read_markers::save_read_offset(
                path,
                self.current_server.as_deref(),
                target,
                self.message_scroll_offset,
            );
        }
    }

    /// Restore read position for target from read_markers.
    pub fn restore_read_marker_for(&mut self, server: &str, target: &str) {
        if target.is_empty() {
            return;
        }
        if let Some(ref path) = self.read_markers_path {
            if let Some(offset) = crate::read_markers::load_read_offset(
                path,
                Some(server),
                target,
            ) {
                self.message_scroll_offset = offset;
            }
        }
    }

    /// True if text contains any highlight word (case-insensitive substring).
    pub fn highlight_matches(&self, text: &str) -> bool {
        if self.highlight_words.is_empty() {
            return false;
        }
        let t = text.to_lowercase();
        self.highlight_words
            .iter()
            .any(|w| !w.is_empty() && t.contains(&w.to_lowercase()))
    }

    /// Set channels for a server. Replaces existing.
    #[allow(dead_code)]
    pub fn set_server_channels(&mut self, server: &str, channels: Vec<String>) {
        self.channels_per_server.insert(server.to_string(), channels);
        self.clamp_channel_index();
    }

    pub fn set_user_list(&mut self, server: &str, users: Vec<String>, userhosts: Vec<(String, String)>) {
        self.user_list = users;
        for (nick, userhost) in userhosts {
            self.userhost_per_nick.insert((server.to_string(), nick), userhost);
        }
        self.sort_user_list();
        if self.user_index >= self.user_list.len() && !self.user_list.is_empty() {
            self.user_index = self.user_list.len().saturating_sub(1);
        }
    }

    /// Set user list for a DM: just the DM target and ourselves.
    pub fn set_user_list_for_dm(&mut self, dm_nick: &str) {
        let mut list = vec![dm_nick.to_string()];
        if let Some(ref our_nick) = self.current_nickname {
            if !our_nick.eq_ignore_ascii_case(dm_nick) {
                list.push(our_nick.clone());
            }
        }
        self.user_list = list;
        self.user_list_filter.clear();
        self.user_index = 0;
    }

    /// Display title for the current message target (for Messages window title).
    pub fn current_target_title(&self) -> String {
        match self.current_channel.as_deref() {
            None => "*".to_string(),
            Some("*server*") => self
                .current_server
                .as_deref()
                .unwrap_or("Server")
                .to_string(),
            Some(t) => t.to_string(),
        }
    }

    /// Nick without channel prefix (for commands: /msg, whois, etc.).
    pub fn selected_user(&self) -> Option<String> {
        self.filtered_user_list()
            .get(self.user_index)
            .map(|s| Self::strip_user_prefix(s).to_string())
    }

    /// Strip channel membership prefix (@, %, +, ~, &, !, .) from a user list entry.
    pub fn strip_user_prefix(s: &str) -> &str {
        s.trim_start_matches(|c: char| "@%+~&!.".contains(c))
    }

    /// Role rank for sorting: lower = higher privilege. Order: @ ~ ! % & . + (none).
    fn user_role_rank(entry: &str) -> u8 {
        match entry.chars().next() {
            Some('@') => 0,
            Some('~') => 1,
            Some('!') => 2,
            Some('%') => 3,
            Some('&') => 4,
            Some('.') => 5,
            Some('+') => 6,
            _ => 7,
        }
    }

    /// Sort user list by role (highest first) then alphabetically by nick (case-insensitive).
    pub fn sort_user_list(&mut self) {
        self.user_list.sort_by(|a, b| {
            let r = Self::user_role_rank(a).cmp(&Self::user_role_rank(b));
            if r != std::cmp::Ordering::Equal {
                return r;
            }
            let na = Self::strip_user_prefix(a).to_lowercase();
            let nb = Self::strip_user_prefix(b).to_lowercase();
            na.cmp(&nb)
        });
    }

    pub fn user_actions() -> &'static [UserAction] {
        &[
            UserAction::Dm,
            UserAction::Kick,
            UserAction::Ban,
            UserAction::Unban,
            UserAction::Op,
            UserAction::Deop,
            UserAction::Voice,
            UserAction::Devoice,
            UserAction::Halfop,
            UserAction::Dehalfop,
            UserAction::Mute,
            UserAction::Whois,
        ]
    }

    /// Clamp channel_index to valid range for channels_list.
    pub fn clamp_channel_index(&mut self) {
        let len = self.channels_list().len();
        if len == 0 {
            self.channel_index = 0;
        } else {
            self.channel_index = self.channel_index.min(len - 1);
        }
    }

    /// Clamp messages_index to valid range for messages_list.
    pub fn clamp_messages_index(&mut self) {
        let len = self.messages_list().len();
        if len == 0 {
            self.messages_index = 0;
        } else {
            self.messages_index = self.messages_index.min(len - 1);
        }
    }

    /// Users pane: filter by nick (case-insensitive substring). Strips role prefixes for matching.
    pub fn filtered_user_list(&self) -> Vec<String> {
        let q = self.user_list_filter.to_lowercase();
        if q.is_empty() {
            return self.user_list.clone();
        }
        self.user_list
            .iter()
            .filter(|u| {
                let nick = u.trim_start_matches(|c: char| matches!(c, '~' | '&' | '@' | '%' | '+' | '!' | '.'));
                nick.to_lowercase().contains(&q)
            })
            .cloned()
            .collect()
    }

    /// Clamp user_index to filtered user list length.
    pub fn clamp_user_index(&mut self) {
        let len = self.filtered_user_list().len();
        if len == 0 {
            self.user_index = 0;
        } else {
            self.user_index = self.user_index.min(len - 1);
        }
    }

    /// Clamp friends_index to valid range for visible friends list.
    pub fn clamp_friends_index(&mut self) {
        let len = self.visible_friends().len();
        if len == 0 {
            self.friends_index = 0;
        } else {
            self.friends_index = self.friends_index.min(len - 1);
        }
    }

    /// All friends from all connected servers, deduped and sorted.
    fn all_friends_sorted(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for server in &self.connected_servers {
            if let Some(friends) = self.friends_per_server.get(server) {
                for n in friends {
                    if seen.insert(n.to_lowercase()) {
                        out.push(n.clone());
                    }
                }
            }
        }
        out.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        out
    }

    /// Friends list filtered by offline_friends: hide = only online, show = all.
    pub fn visible_friends(&self) -> Vec<String> {
        let all = self.all_friends_sorted();
        let hide = self
            .offline_friends
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("hide"))
            .unwrap_or(false);
        if hide {
            all.into_iter()
                .filter(|n| self.is_friend_online_anywhere(n))
                .collect()
        } else {
            all
        }
    }

    /// True if nick is a friend on any server.
    #[allow(dead_code)]
    pub fn is_friend(&self, nick: &str) -> bool {
        self.friends_per_server
            .values()
            .any(|v| v.iter().any(|n| n.eq_ignore_ascii_case(nick)))
    }

    /// Friend status for display: (online, away). Checks all servers where nick is a friend.
    pub fn friend_status(&self, nick: &str) -> (bool, bool) {
        let online = self.is_friend_online_anywhere(nick);
        let away = self.is_friend_away_anywhere(nick);
        (online, away)
    }

    /// True if nick is a friend on the given server.
    pub fn is_friend_on_server(&self, server: &str, nick: &str) -> bool {
        self.friends_per_server
            .get(server)
            .map_or(false, |v| v.iter().any(|n| n.eq_ignore_ascii_case(nick)))
    }

    /// True if nick is online on any server where they're a friend.
    fn is_friend_online_anywhere(&self, nick: &str) -> bool {
        self.friends_per_server.iter().any(|(server, friends)| {
            friends.iter().any(|f| f.eq_ignore_ascii_case(nick))
                && self
                    .friends_online
                    .get(server)
                    .map_or(false, |o| o.iter().any(|x| x.eq_ignore_ascii_case(nick)))
        })
    }

    /// True if nick is away on any server where they're a friend.
    fn is_friend_away_anywhere(&self, nick: &str) -> bool {
        self.friends_per_server.iter().any(|(server, friends)| {
            friends.iter().any(|f| f.eq_ignore_ascii_case(nick))
                && self
                    .friends_away
                    .get(server)
                    .map_or(false, |a| a.iter().any(|x| x.eq_ignore_ascii_case(nick)))
        })
    }

    /// Set channel_index or messages_index to match current_server+current_channel.
    pub fn sync_channel_index_to_current(&mut self) {
        let Some(ref server) = self.current_server else { return };
        let Some(ref target) = self.current_channel else { return };
        if target == "*server*" || target.starts_with('#') || target.starts_with('&') {
            if let Some(pos) = self.channels_list().iter().position(|(s, t)| s == server && t == target) {
                self.channel_index = pos;
            }
        } else {
            if let Some(pos) = self.messages_list().iter().position(|(s, n)| s == server && n == target) {
                self.messages_index = pos;
            }
        }
    }

    /// Selected friend from the friends pane (from visible list).
    pub fn selected_friend(&self) -> Option<String> {
        self.visible_friends().get(self.friends_index).cloned()
    }

    /// Messages editable by us: our own messages with a msgid, in chronological order.
    /// Returns (msgid, text) pairs.
    pub fn editable_msgids(&self) -> Vec<(String, String)> {
        let our_nick = self.current_nickname.as_deref().unwrap_or("");
        let target_key = msg_key(
            self.current_server.as_deref().unwrap_or(""),
            self.current_channel.as_deref().unwrap_or("*server*"),
        );
        self.current_messages()
            .iter()
            .filter(|m| {
                m.msgid.is_some()
                    && m.source.eq_ignore_ascii_case(our_nick)
                    && !self.is_muted(&target_key, &m.source)
            })
            .filter_map(|m| m.msgid.as_ref().map(|id| (id.clone(), m.text.clone())))
            .collect()
    }

    /// msgid -> display number (1–9, 10 for "0") for edit-select mode. Only last 10 editable.
    pub fn edit_select_numbers(&self) -> std::collections::HashMap<String, u8> {
        let ids = self.editable_msgids();
        let len = ids.len();
        let mut map = std::collections::HashMap::new();
        for (i, (msgid, _)) in ids.iter().enumerate().rev().take(10) {
            let num = (len - i) as u8;
            map.insert(msgid.clone(), num);
        }
        map
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
