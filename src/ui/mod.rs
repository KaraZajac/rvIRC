//! UI: layout, status bar, message area, input bar, channel/user panes.

mod layout;

use crate::app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

const CHANNELS_PANE_WIDTH: u16 = 22;
const USERS_PANE_WIDTH: u16 = 18;

/// Popup style: use terminal default background so popups match the terminal.
fn popup_overlay_style() -> Style {
    Style::default()
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    // Vertical: messages (dynamic), input (3 lines), status (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let main_area = chunks[0];
    let input_area = chunks[1];
    let status_area = chunks[2];

    let (center, left_opt, right_opt) = layout::center_with_side_panes(
        main_area,
        app.channel_panel_visible.then_some(CHANNELS_PANE_WIDTH),
        app.user_panel_visible.then_some(USERS_PANE_WIDTH),
    );

    if let Some(left) = left_opt {
        draw_channels_pane(f, left, app);
    }
    draw_message_area(f, center, app);
    if let Some(right) = right_opt {
        draw_users_pane(f, right, app);
    }

    draw_input_bar(f, input_area, app);
    draw_status_bar(f, status_area, app);

    if app.user_action_menu {
        if let Some(ref nick) = app.selected_user() {
            draw_user_action_menu(f, app, nick);
        }
    }

    if app.channel_list_popup_visible {
        draw_channel_list_popup(f, area, app);
    }

    if app.server_list_popup_visible {
        draw_server_list_popup(f, area, app);
    }

    if app.whois_popup_visible {
        draw_whois_popup(f, area, app);
    }
}

fn draw_message_area(f: &mut Frame, area: Rect, app: &App) {
    let messages = app.current_messages();
    let all_lines: Vec<Line> = messages
        .iter()
        .map(|m| format_message_line(m))
        .collect();
    let visible_rows = area.height.saturating_sub(2) as usize;
    let (start, end) = if all_lines.len() <= visible_rows {
        (0, all_lines.len())
    } else {
        let start = all_lines
            .len()
            .saturating_sub(visible_rows)
            .saturating_sub(app.message_scroll_offset)
            .max(0);
        let end = all_lines.len().saturating_sub(app.message_scroll_offset).min(all_lines.len());
        (start, end)
    };
    let lines: Vec<Line> = all_lines.get(start..end).unwrap_or(&[]).to_vec();
    let title = app.current_target_title();
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(format!(" {} ", title)))
        .wrap(Wrap { trim: true })
        .style(Style::default());
    f.render_widget(paragraph, area);
}

fn format_message_line(m: &MessageLine) -> Line<'_> {
    let (prefix, style) = match m.kind {
        MessageKind::Privmsg => (format!("<{}> ", m.source), Style::default()),
        MessageKind::Notice => (format!("[{}] ", m.source), Style::default().add_modifier(Modifier::ITALIC)),
        MessageKind::Join => (format!("*** {} ", m.source), Style::default().fg(Color::Cyan)),
        MessageKind::Part | MessageKind::Quit => (format!("*** {} ", m.source), Style::default().fg(Color::Yellow)),
        MessageKind::Nick => (format!("*** {} ", m.source), Style::default().fg(Color::Magenta)),
        MessageKind::Mode => (format!("*** "), Style::default().fg(Color::Green)),
        MessageKind::Other => (format!("{} ", m.source), Style::default()),
    };
    Line::from(vec![
        Span::styled(prefix, style),
        Span::styled(m.text.as_str(), Style::default()),
    ])
}

fn draw_input_bar(f: &mut Frame, area: Rect, app: &App) {
    let prompt = match app.mode {
        Mode::Command => ":",
        _ => "",
    };
    let content = format!("{}{}", prompt, app.input);
    let paragraph = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" Input "))
        .style(Style::default());
    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let (mode_label, mode_style) = match app.mode {
        Mode::Normal => (" NORMAL ", Style::default().fg(Color::Black).bg(Color::Blue)),
        Mode::Insert => (" INSERT ", Style::default().fg(Color::Black).bg(Color::Green)),
        Mode::Command => (" COMMAND ", Style::default().fg(Color::Black).bg(Color::Rgb(255, 165, 0))),
    };
    let right = format!(
        "{} | {}",
        app.current_server.as_deref().unwrap_or(""),
        app.current_channel.as_deref().unwrap_or("*")
    );
    let line = Line::from(vec![
        Span::styled(mode_label, mode_style),
        Span::raw(" "),
        Span::raw(&app.status_message),
        Span::raw(" "),
        Span::raw(right),
    ]);
    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}

fn target_display_label(app: &App, target: &str) -> String {
    if target == "*server*" {
        app.current_server
            .as_deref()
            .unwrap_or("Server")
            .to_string()
    } else {
        target.to_string()
    }
}

fn draw_channels_pane(f: &mut Frame, area: Rect, app: &App) {
    let show_selector = app.panel_focus == PanelFocus::Channels;
    let target_list = app.target_list();
    let items: Vec<ListItem> = target_list
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let label = target_display_label(app, t);
            let line = if show_selector && i == app.channel_index {
                format!("> {}  ", label)
            } else {
                format!("  {}  ", label)
            };
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Channels "))
        .style(Style::default());
    f.render_widget(list, area);
}

/// Style for a user list entry based on channel prefix (op, halfop, voice, etc.).
fn user_prefix_style(entry: &str) -> Style {
    let first = entry.chars().next();
    match first {
        Some('@') => Style::default().fg(Color::Red),           // channel op
        Some('%') => Style::default().fg(Color::Yellow),         // halfop
        Some('+') => Style::default().fg(Color::Green),          // voice
        Some('~') => Style::default().fg(Color::Magenta),        // founder
        Some('&') => Style::default().fg(Color::Cyan),            // protected
        Some('!') => Style::default().fg(Color::Blue),           // admin
        Some('.') => Style::default().fg(Color::Cyan),            // owner
        _ => Style::default(),
    }
}

fn draw_users_pane(f: &mut Frame, area: Rect, app: &App) {
    let show_selector = app.panel_focus == PanelFocus::Users;
    let items: Vec<ListItem> = app
        .user_list
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let prefix = if show_selector && i == app.user_index { "> " } else { "  " };
            let role_style = user_prefix_style(u);
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(u.as_str(), role_style),
                Span::raw("  "),
            ]);
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Users "))
        .style(Style::default());
    let visible = area.height.saturating_sub(2) as usize;
    let offset = if app.user_list.len() <= visible {
        0
    } else {
        (app.user_index + 1).saturating_sub(visible).min(app.user_list.len() - visible).max(0)
    };
    let mut state = ListState::default().with_selected(Some(app.user_index)).with_offset(offset);
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_user_action_menu(f: &mut Frame, app: &App, nick: &str) {
    let area = f.area();
    let menu_width = 26;
    let menu_height = 10;
    let x = area.width.saturating_sub(menu_width).saturating_sub(2) / 2;
    let y = area.height.saturating_sub(menu_height) / 2;
    let menu_rect = Rect {
        x,
        y,
        width: menu_width,
        height: menu_height,
    };
    let actions = App::user_actions();
    let items: Vec<ListItem> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let label = action_label(a);
            let line = if i == app.user_action_index {
                format!("> {}  ", label)
            } else {
                format!("  {}  ", label)
            };
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", nick)),
        )
        .style(Style::default());
    f.render_widget(list, menu_rect);
}

fn draw_channel_list_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(60).max(30);
    let popup_height = (area.height * 3 / 4).min(24).max(10);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect {
        x,
        y,
        width: popup_width,
        height: popup_height,
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .margin(1)
        .split(popup_rect);

    let filtered = app.filtered_server_channel_list();
    let list_items: Vec<ListItem> = if filtered.is_empty() {
        let msg = if app.server_channel_list.is_empty() {
            "Loading..."
        } else {
            "No channels match filter"
        };
        vec![ListItem::new(format!("  {}  ", msg))]
    } else {
        filtered
            .iter()
            .enumerate()
            .map(|(i, (name, count))| {
                let label = match count {
                    Some(n) => format!("{} ({})", name, n),
                    None => name.clone(),
                };
                let line = if i == app.channel_list_selected_index {
                    format!("> {}  ", label)
                } else {
                    format!("  {}  ", label)
                };
                ListItem::new(line)
            })
            .collect()
    };

    let filter_display = if app.channel_list_filter.is_empty() {
        "Filter: (type to search)".to_string()
    } else {
        format!("Filter: {}", app.channel_list_filter)
    };
    let mode_hint = if app.channel_list_scroll_mode {
        "j/k or arrows: move | Enter: join | Esc: back to search"
    } else {
        "Type to search (j,k work here) | Enter: browse list | Esc: close"
    };

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Channel list ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let filter_para = Paragraph::new(filter_display).style(popup_style);
    f.render_widget(filter_para, chunks[0]);
    let hint_para = Paragraph::new(mode_hint).style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint_para, chunks[1]);

    let list = List::new(list_items).style(popup_style);
    let list_area = chunks[2];
    let visible = list_area.height as usize;
    let filtered_len = filtered.len();
    let offset = if filtered_len <= visible || visible == 0 {
        0
    } else {
        (app.channel_list_selected_index + 1)
            .saturating_sub(visible)
            .min(filtered_len - visible)
            .max(0)
    };
    let mut list_state = ListState::default()
        .with_selected(Some(app.channel_list_selected_index))
        .with_offset(offset);
    f.render_stateful_widget(list, list_area, &mut list_state);
}

fn draw_server_list_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(44).max(24);
    let popup_height = (area.height * 3 / 4).min(20).max(8);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect {
        x,
        y,
        width: popup_width,
        height: popup_height,
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3)])
        .margin(1)
        .split(popup_rect);

    let list_items: Vec<ListItem> = if app.server_list.is_empty() {
        vec![ListItem::new("  (no servers in config)  ")]
    } else {
        app.server_list
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let line = if i == app.server_list_selected_index {
                    format!("> {}  ", name)
                } else {
                    format!("  {}  ", name)
                };
                ListItem::new(line)
            })
            .collect()
    };

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Servers ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let list = List::new(list_items).style(popup_style);
    let list_area = chunks[0];
    let visible = list_area.height as usize;
    let len = app.server_list.len();
    let offset = if len <= visible || visible == 0 {
        0
    } else {
        (app.server_list_selected_index + 1)
            .saturating_sub(visible)
            .min(len.saturating_sub(visible))
            .max(0)
    };
    let mut list_state = ListState::default()
        .with_selected(if app.server_list.is_empty() {
            None
        } else {
            Some(app.server_list_selected_index)
        })
        .with_offset(offset);
    f.render_stateful_widget(list, list_area, &mut list_state);
}

fn draw_whois_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(56).max(28);
    let popup_height = (area.height * 3 / 4).min(20).max(8);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect {
        x,
        y,
        width: popup_width,
        height: popup_height,
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2)])
        .margin(1)
        .split(popup_rect);

    let title = if app.whois_nick.is_empty() {
        " Whois ".to_string()
    } else {
        format!(" Whois: {} ", app.whois_nick)
    };
    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let lines: Vec<ListItem> = if app.whois_lines.is_empty() {
        vec![ListItem::new("  (no data)  ")]
    } else {
        app.whois_lines
            .iter()
            .map(|s| ListItem::new(format!("  {}  ", s)))
            .collect()
    };
    let list = List::new(lines).style(popup_style);
    f.render_widget(list, chunks[0]);

    let hint = Paragraph::new("Esc / Enter / q to close").style(popup_style.add_modifier(Modifier::DIM));
    let hint_rect = Rect {
        x: popup_rect.x,
        y: popup_rect.y + popup_rect.height.saturating_sub(1),
        width: popup_rect.width,
        height: 1,
    };
    f.render_widget(hint, hint_rect);
}

fn action_label(a: &UserAction) -> &'static str {
    match a {
        UserAction::Dm => "Direct message",
        UserAction::Kick => "Kick",
        UserAction::Ban => "Ban",
        UserAction::Mute => "Mute",
        UserAction::Whois => "Whois",
    }
}
