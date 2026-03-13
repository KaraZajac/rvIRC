//! Key handling and mode transitions.

use crate::app::{Mode, PanelFocus};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(
    event: Event,
    mode: Mode,
    panel_focus: PanelFocus,
    channel_panel_visible: bool,
    messages_panel_visible: bool,
    user_panel_visible: bool,
    friends_panel_visible: bool,
    user_action_menu: bool,
    channel_list_popup_visible: bool,
    channel_list_scroll_mode: bool,
    server_list_popup_visible: bool,
    whois_popup_visible: bool,
    credits_popup_visible: bool,
    license_popup_visible: bool,
    file_receive_popup_visible: bool,
    file_browser_visible: bool,
    secure_accept_popup_visible: bool,
) -> Option<KeyAction> {
    let key = match event {
        Event::Key(k) => k,
        _ => return None,
    };

    if file_browser_visible {
        return Some(handle_file_browser(key));
    }
    if secure_accept_popup_visible {
        return Some(handle_secure_accept_popup(key));
    }
    if file_receive_popup_visible {
        return Some(handle_file_receive_popup(key));
    }
    if credits_popup_visible {
        return Some(handle_credits_popup(key));
    }
    if license_popup_visible {
        return Some(handle_license_popup(key));
    }
    if whois_popup_visible {
        return Some(handle_whois_popup(key));
    }
    if channel_list_popup_visible {
        return Some(handle_list_popup(key, channel_list_scroll_mode));
    }
    if server_list_popup_visible {
        return Some(handle_server_list_popup(key));
    }
    if user_action_menu {
        return Some(handle_user_action_menu(key));
    }
    if mode == Mode::Normal && panel_focus == PanelFocus::Channels && channel_panel_visible {
        return Some(handle_channels_pane(key));
    }
    if mode == Mode::Normal && panel_focus == PanelFocus::Messages && messages_panel_visible {
        return Some(handle_messages_pane(key));
    }
    if mode == Mode::Normal && panel_focus == PanelFocus::Users && user_panel_visible {
        return Some(handle_users_pane(key));
    }
    if mode == Mode::Normal && panel_focus == PanelFocus::Friends && friends_panel_visible {
        return Some(handle_friends_pane(key));
    }

    match mode {
        Mode::Normal => handle_normal(key, panel_focus),
        Mode::Insert => handle_insert(key),
        Mode::Command => handle_command(key),
    }
}

#[derive(Debug, Clone)]
pub enum KeyAction {
    NoOp,
    QuitApp,
    SwitchMode(Mode),
    FocusChannels,
    FocusMessages,
    FocusUsers,
    FocusFriends,
    UnfocusPanel,
    ChannelUp,
    ChannelDown,
    ChannelSelect,
    MessageUp,
    MessageDown,
    MessageSelect,
    FriendUp,
    FriendDown,
    FriendSelect,
    UserUp,
    UserDown,
    UserSelect,
    UserActionMenuUp,
    UserActionMenuDown,
    UserActionConfirm,
    CloseUserActionMenu,
    ListPopupUp,
    ListPopupDown,
    ListPopupSelect,
    ListPopupClose,
    ListPopupFocusList,
    ListPopupFocusFilter,
    ListPopupFilterChar(char),
    ListPopupBackspace,
    CloseWhoisPopup,
    CloseCreditsPopup,
    CloseLicensePopup,
    LicenseScrollUp,
    LicenseScrollDown,
    LicenseScrollPageUp,
    LicenseScrollPageDown,
    ServerListPopupUp,
    ServerListPopupDown,
    ServerListPopupSelect,
    ServerListPopupClose,
    MessageScrollUp,
    MessageScrollDown,
    MessageScrollPageUp,
    MessageScrollPageDown,
    Char(char),
    Backspace,
    Enter,
    Esc,
    InputHistoryUp,
    InputHistoryDown,
    TabComplete,
    SecureAccept,
    SecureReject,
    FileReceiveAccept,
    FileReceiveReject,
    FileBrowserUp,
    FileBrowserDown,
    FileBrowserEnter,
    FileBrowserBack,
    FileBrowserSelect,
    FileBrowserClose,
}

fn handle_normal(key: KeyEvent, panel_focus: PanelFocus) -> Option<KeyAction> {
    if panel_focus == PanelFocus::Main {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => return Some(KeyAction::MessageScrollUp),
            KeyCode::Down | KeyCode::Char('j') => return Some(KeyAction::MessageScrollDown),
            KeyCode::PageUp => return Some(KeyAction::MessageScrollPageUp),
            KeyCode::PageDown => return Some(KeyAction::MessageScrollPageDown),
            _ => {}
        }
    }
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(KeyAction::QuitApp),
        (KeyCode::Char('i'), KeyModifiers::NONE) => Some(KeyAction::SwitchMode(Mode::Insert)),
        (KeyCode::Char(':'), KeyModifiers::NONE) => Some(KeyAction::SwitchMode(Mode::Command)),
        (KeyCode::Char('c'), KeyModifiers::NONE) => Some(KeyAction::FocusChannels),
        (KeyCode::Char('m'), KeyModifiers::NONE) => Some(KeyAction::FocusMessages),
        (KeyCode::Char('u'), KeyModifiers::NONE) => Some(KeyAction::FocusUsers),
        (KeyCode::Char('f'), KeyModifiers::NONE) => Some(KeyAction::FocusFriends),
        (KeyCode::Esc, _) if panel_focus != PanelFocus::Main => Some(KeyAction::UnfocusPanel),
        _ => None,
    }
}

fn handle_insert(key: KeyEvent) -> Option<KeyAction> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(KeyAction::QuitApp),
        (KeyCode::Esc, _) => Some(KeyAction::Esc),
        (KeyCode::Enter, _) => Some(KeyAction::Enter),
        (KeyCode::Backspace, _) => Some(KeyAction::Backspace),
        (KeyCode::Up, _) => Some(KeyAction::InputHistoryUp),
        (KeyCode::Down, _) => Some(KeyAction::InputHistoryDown),
        (KeyCode::Tab, _) => Some(KeyAction::TabComplete),
        (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT) => Some(KeyAction::Char(c)),
        _ => None,
    }
}

fn handle_command(key: KeyEvent) -> Option<KeyAction> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(KeyAction::QuitApp),
        (KeyCode::Esc, _) => Some(KeyAction::Esc),
        (KeyCode::Enter, _) => Some(KeyAction::Enter),
        (KeyCode::Backspace, _) => Some(KeyAction::Backspace),
        (KeyCode::Up, _) => Some(KeyAction::InputHistoryUp),
        (KeyCode::Down, _) => Some(KeyAction::InputHistoryDown),
        (KeyCode::Tab, _) => Some(KeyAction::TabComplete),
        (KeyCode::Char(c), _) => Some(KeyAction::Char(c)),
        _ => None,
    }
}

fn handle_channels_pane(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('c') | KeyCode::Esc => KeyAction::UnfocusPanel,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::ChannelUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::ChannelDown,
        KeyCode::Enter => KeyAction::ChannelSelect,
        _ => KeyAction::NoOp,
    }
}

fn handle_messages_pane(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('m') | KeyCode::Esc => KeyAction::UnfocusPanel,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::MessageUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::MessageDown,
        KeyCode::Enter => KeyAction::MessageSelect,
        _ => KeyAction::NoOp,
    }
}

fn handle_users_pane(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('u') | KeyCode::Esc => KeyAction::UnfocusPanel,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::UserUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::UserDown,
        KeyCode::Enter => KeyAction::UserSelect,
        _ => KeyAction::NoOp,
    }
}

fn handle_friends_pane(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('f') | KeyCode::Esc => KeyAction::UnfocusPanel,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::FriendUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::FriendDown,
        KeyCode::Enter => KeyAction::FriendSelect,
        _ => KeyAction::NoOp,
    }
}

fn handle_user_action_menu(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => KeyAction::CloseUserActionMenu,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::UserActionMenuUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::UserActionMenuDown,
        KeyCode::Enter => KeyAction::UserActionConfirm,
        _ => KeyAction::NoOp,
    }
}

fn handle_whois_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => KeyAction::CloseWhoisPopup,
        _ => KeyAction::NoOp,
    }
}

fn handle_credits_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => KeyAction::CloseCreditsPopup,
        _ => KeyAction::NoOp,
    }
}

fn handle_license_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => KeyAction::CloseLicensePopup,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::LicenseScrollUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::LicenseScrollDown,
        KeyCode::PageUp => KeyAction::LicenseScrollPageUp,
        KeyCode::PageDown => KeyAction::LicenseScrollPageDown,
        _ => KeyAction::NoOp,
    }
}

fn handle_server_list_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Esc => KeyAction::ServerListPopupClose,
        KeyCode::Enter => KeyAction::ServerListPopupSelect,
        KeyCode::Up | KeyCode::Char('k') => KeyAction::ServerListPopupUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::ServerListPopupDown,
        _ => KeyAction::NoOp,
    }
}

fn handle_secure_accept_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => KeyAction::SecureAccept,
        KeyCode::Char('n') | KeyCode::Esc => KeyAction::SecureReject,
        _ => KeyAction::NoOp,
    }
}

fn handle_file_receive_popup(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => KeyAction::FileReceiveAccept,
        KeyCode::Char('n') | KeyCode::Esc => KeyAction::FileReceiveReject,
        _ => KeyAction::NoOp,
    }
}

fn handle_file_browser(key: KeyEvent) -> KeyAction {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => KeyAction::FileBrowserUp,
        KeyCode::Down | KeyCode::Char('j') => KeyAction::FileBrowserDown,
        KeyCode::Enter => KeyAction::FileBrowserEnter,
        KeyCode::Backspace => KeyAction::FileBrowserBack,
        KeyCode::Char('s') => KeyAction::FileBrowserSelect,
        KeyCode::Esc | KeyCode::Char('q') => KeyAction::FileBrowserClose,
        _ => KeyAction::NoOp,
    }
}

fn handle_list_popup(key: KeyEvent, scroll_mode: bool) -> KeyAction {
    use crossterm::event::KeyModifiers;
    if scroll_mode {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => KeyAction::ListPopupFocusFilter,
            (KeyCode::Enter, _) => KeyAction::ListPopupSelect,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => KeyAction::ListPopupUp,
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => KeyAction::ListPopupDown,
            _ => KeyAction::NoOp,
        }
    } else {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => KeyAction::ListPopupClose,
            (KeyCode::Enter, _) => KeyAction::ListPopupFocusList,
            (KeyCode::Backspace, _) => KeyAction::ListPopupBackspace,
            (KeyCode::Char(c), KeyModifiers::NONE) | (KeyCode::Char(c), KeyModifiers::SHIFT) => KeyAction::ListPopupFilterChar(c),
            _ => KeyAction::NoOp,
        }
    }
}
