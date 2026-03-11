//! App state: mode, channels, users, message buffers, input, pane visibility.

use std::collections::HashMap;
use std::time::Instant;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Privmsg,
    Notice,
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
    Mute,
    Whois,
}

/// Which panel has focus in Normal mode (keyboard input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelFocus {
    Main,
    Channels,
    Users,
}

pub struct App {
    pub mode: Mode,
    pub input: String,
    pub input_cursor: usize,

    pub channel_panel_visible: bool,
    pub channel_list: Vec<String>,
    pub channel_index: usize,

    pub user_panel_visible: bool,
    pub user_list: Vec<String>,
    pub user_index: usize,
    pub user_action_menu: bool,
    pub user_action_index: usize,

    pub panel_focus: PanelFocus,

    pub current_channel: Option<String>,
    pub current_server: Option<String>,
    pub current_nickname: Option<String>,
    pub dm_targets: Vec<String>,
    pub messages: HashMap<String, Vec<MessageLine>>,

    /// :list popup: server channel list (from LIST command), filter, selection.
    /// Each entry is (channel name, optional user count).
    pub channel_list_popup_visible: bool,
    pub server_channel_list: Vec<(String, Option<u32>)>,
    pub channel_list_filter: String,
    pub channel_list_selected_index: usize,
    /// false = filter mode (type to search), true = scroll mode (j/k/arrows move, Enter joins).
    pub channel_list_scroll_mode: bool,

    /// :servers popup: list of server names from config, select one to connect.
    pub server_list_popup_visible: bool,
    pub server_list: Vec<String>,
    pub server_list_selected_index: usize,

    /// Whois popup: show result when user selects Whois from user action menu.
    pub whois_popup_visible: bool,
    pub whois_nick: String,
    pub whois_lines: Vec<String>,

    /// Message area scroll: 0 = show latest; increase when user scrolls up (see older).
    pub message_scroll_offset: usize,

    /// Set when Connected is received; main loop uses this to run auto_join then clears it.
    pub pending_auto_join: bool,
    /// When set, auto_join runs only after this time (identify first, then join channels).
    pub auto_join_after: Option<Instant>,

    pub status_message: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            channel_panel_visible: true,
            channel_list: Vec::new(),
            channel_index: 0,
            user_panel_visible: true,
            user_list: Vec::new(),
            user_index: 0,
            user_action_menu: false,
            user_action_index: 0,
            panel_focus: PanelFocus::Main,
            current_channel: None,
            current_server: None,
            current_nickname: None,
            dm_targets: Vec::new(),
            messages: HashMap::new(),
            channel_list_popup_visible: false,
            server_channel_list: Vec::new(),
            channel_list_filter: String::new(),
            channel_list_selected_index: 0,
            channel_list_scroll_mode: false,
            server_list_popup_visible: false,
            server_list: Vec::new(),
            server_list_selected_index: 0,
            whois_popup_visible: false,
            whois_nick: String::new(),
            whois_lines: Vec::new(),
            message_scroll_offset: 0,
            pending_auto_join: false,
            auto_join_after: None,
            status_message: String::new(),
        }
    }

    /// Filtered server channel list for the :list popup (substring match on name, case-insensitive).
    pub fn filtered_server_channel_list(&self) -> Vec<(String, Option<u32>)> {
        let q = self.channel_list_filter.to_lowercase();
        if q.is_empty() {
            return self.server_channel_list.clone();
        }
        self.server_channel_list
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(&q))
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

    /// Selected channel name in the list popup (from filtered list), if any.
    pub fn selected_list_channel(&self) -> Option<String> {
        let filtered = self.filtered_server_channel_list();
        filtered.get(self.channel_list_selected_index).map(|(name, _)| name.clone())
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

    /// Ordered list for the left panel: server (if connected), then channels, then DM nicks.
    pub fn target_list(&self) -> Vec<String> {
        let mut list = Vec::new();
        if self.current_server.is_some() {
            list.push("*server*".to_string());
        }
        list.extend(self.channel_list.iter().cloned());
        list.extend(self.dm_targets.iter().cloned());
        list
    }

    /// Selected target from the panel (server, channel, or nick). Clamps index to list len.
    pub fn selected_target(&self) -> Option<String> {
        let list = self.target_list();
        let idx = self.channel_index.min(list.len().saturating_sub(1));
        list.get(idx).cloned()
    }

    pub fn current_messages(&self) -> &[MessageLine] {
        let key = self
            .current_channel
            .as_deref()
            .unwrap_or("*server*");
        self.messages
            .get(key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn push_message(&mut self, target: &str, line: MessageLine) {
        self.messages
            .entry(target.to_string())
            .or_default()
            .push(line);
    }

    #[allow(dead_code)]
    pub fn set_channel_list(&mut self, channels: Vec<String>) {
        self.channel_list = channels;
        if self.channel_index >= self.channel_list.len() && !self.channel_list.is_empty() {
            self.channel_index = self.channel_list.len().saturating_sub(1);
        }
    }

    pub fn set_user_list(&mut self, users: Vec<String>) {
        self.user_list = users;
        self.sort_user_list();
        if self.user_index >= self.user_list.len() && !self.user_list.is_empty() {
            self.user_index = self.user_list.len().saturating_sub(1);
        }
    }

    pub fn selected_channel(&self) -> Option<String> {
        self.selected_target()
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
        self.user_list
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
            UserAction::Mute,
            UserAction::Whois,
        ]
    }

    /// Clamp channel_index to valid range for target_list (e.g. after Part or Disconnect).
    pub fn clamp_channel_index(&mut self) {
        let len = self.target_list().len();
        if len == 0 {
            self.channel_index = 0;
        } else {
            self.channel_index = self.channel_index.min(len - 1);
        }
    }

    /// Set channel_index to the index of current_channel in target_list (e.g. after Join or SwitchChannel).
    pub fn sync_channel_index_to_current(&mut self) {
        let list = self.target_list();
        if let Some(ref target) = self.current_channel {
            if let Some(pos) = list.iter().position(|t| t == target) {
                self.channel_index = pos;
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
