//! UI: layout, status bar, message area, input bar, channel/user panes.

mod layout;

use crate::app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

const CHANNELS_PANE_WIDTH: u16 = 22;
const USERS_PANE_WIDTH: u16 = 18;

/// Width in terminal cells of the input bar content (prompt + input + cursor if shown).
fn input_content_width(app: &App, show_cursor: bool) -> usize {
    let prompt = if app.mode == Mode::Command { ":" } else { "" };
    let before = &app.input[..app.input_cursor.min(app.input.len())];
    let after = app.input.get(app.input_cursor..).unwrap_or("");
    let cursor_cell = if show_cursor { 1 } else { 0 };
    prompt.width() + before.width() + cursor_cell + after.width()
}

/// Number of lines the input content would wrap to at the given inner width (excluding borders).
fn input_wrapped_line_count(app: &App, inner_width: u16) -> usize {
    let show_cursor = app.mode == Mode::Insert || app.mode == Mode::Command;
    let w = input_content_width(app, show_cursor);
    let width = inner_width as usize;
    if width == 0 {
        return 1;
    }
    ((w + width - 1) / width).max(1)
}

/// Popup style: use terminal default background so popups match the terminal.
fn popup_overlay_style() -> Style {
    Style::default()
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    // Input height grows with wrapped lines (1–10 content lines + 2 borders), resets when message is sent
    let input_inner_width = area.width.saturating_sub(2);
    let input_content_lines = input_wrapped_line_count(app, input_inner_width);
    let input_height = (input_content_lines + 2).min(12).max(3) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_height),
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

    if app.credits_popup_visible {
        draw_credits_popup(f, area);
    }

    if app.license_popup_visible {
        draw_license_popup(f, area, app);
    }
}

fn draw_message_area(f: &mut Frame, area: Rect, app: &App) {
    let target_key = app.current_channel.as_deref().unwrap_or("*server*");
    let messages = app.current_messages();
    let all_lines: Vec<Line> = messages
        .iter()
        .filter(|m| !app.is_muted(target_key, &m.source))
        .map(|m| format_message_line(m, app.current_nickname.as_deref()))
        .collect();
    let mut header_lines: Vec<Line> = Vec::new();
    if let Some(topic) = app.current_topic() {
        header_lines.push(Line::from(Span::styled(
            format!(" Topic: {} ", topic),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
        )));
    }
    if let Some(modes) = app.current_modes() {
        header_lines.push(Line::from(Span::styled(
            format!(" Modes: {} ", modes),
            Style::default().fg(Color::Green).add_modifier(Modifier::DIM),
        )));
    }
    let visible_rows = area
        .height
        .saturating_sub(2)
        .saturating_sub(header_lines.len() as u16) as usize;
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
    let mut all_content = header_lines;
    all_content.extend(lines);
    let paragraph = Paragraph::new(all_content)
        .block(Block::default().borders(Borders::ALL).title(format!(" {} ", title)))
        .wrap(Wrap { trim: true })
        .style(Style::default());
    f.render_widget(paragraph, area);
}

fn format_message_line<'a>(m: &'a MessageLine, current_nick: Option<&str>) -> Line<'a> {
    let mention = current_nick.map_or(false, |nick| {
        !nick.is_empty() && m.text.to_lowercase().contains(&nick.to_lowercase())
    });
    let (prefix, style) = match m.kind {
        MessageKind::Privmsg => (
            format!("<{}> ", m.source),
            if mention {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
        MessageKind::Notice => (format!("[{}] ", m.source), Style::default().add_modifier(Modifier::ITALIC)),
        MessageKind::Action => (format!("* {} ", m.source), Style::default().fg(Color::Magenta)),
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
    let show_cursor = app.mode == Mode::Insert || app.mode == Mode::Command;
    let line = if show_cursor {
        let before = format!("{}{}", prompt, &app.input[..app.input_cursor.min(app.input.len())]);
        let after = app.input.get(app.input_cursor..).unwrap_or("");
        Line::from(vec![
            Span::raw(before),
            Span::styled("▌", Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after),
        ])
    } else {
        Line::from(format!("{}{}", prompt, app.input))
    };
    let paragraph = Paragraph::new(line)
        .block(Block::default().borders(Borders::ALL).title(" Input "))
        .wrap(Wrap { trim: true })
        .style(Style::default());
    f.render_widget(paragraph, area);
}

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let (mode_label, mode_style) = match app.mode {
        Mode::Normal => (" NORMAL ", Style::default().fg(Color::Black).bg(Color::Blue)),
        Mode::Insert => (" INSERT ", Style::default().fg(Color::Black).bg(Color::Green)),
        Mode::Command => (" COMMAND ", Style::default().fg(Color::Black).bg(Color::Rgb(255, 165, 0))),
    };
    let target = app.current_channel.as_deref().unwrap_or("*");
    let right = match app.mode {
        Mode::Insert => format!(
            "{} | Sending to: {}",
            app.current_server.as_deref().unwrap_or(""),
            target
        ),
        _ => format!(
            "{} | {}",
            app.current_server.as_deref().unwrap_or(""),
            target
        ),
    };
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
            let line_str = if show_selector && i == app.channel_index {
                format!("> {}  ", label)
            } else {
                format!("  {}  ", label)
            };
            let style = if app.unread_mentions.contains(t) {
                Style::default().fg(Color::Red)
            } else if app.unread_targets.contains(t) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(line_str, style)))
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

const LICENSE_TEXT: &str = include_str!("../../LICENSE");

fn draw_credits_popup(f: &mut Frame, area: Rect) {
    let popup_width = 52;
    let popup_height = 8;
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
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Credits ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let text = "Created by Kara Zajac (.leviathan)\n\nhttps://github.com/KaraZajac";
    let para = Paragraph::new(text)
        .style(popup_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let hint = Paragraph::new("Esc / Enter / q to close").style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn license_wrapped_line_count(inner_width: u16) -> usize {
    let w = inner_width as usize;
    if w == 0 {
        return 1;
    }
    LICENSE_TEXT
        .lines()
        .map(|line| ((line.width() + w - 1) / w).max(1))
        .sum()
}

fn draw_license_popup(f: &mut Frame, area: Rect, app: &mut App) {
    let popup_width = (area.width * 3 / 4).min(72).max(50);
    let popup_height = (area.height * 3 / 4).min(28).max(14);
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
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    let content_area = chunks[0];
    let inner_width = content_area.width;
    let visible_height = content_area.height as usize;
    let total_lines = license_wrapped_line_count(inner_width);
    let max_offset = total_lines.saturating_sub(visible_height).max(0);
    app.license_popup_scroll_offset = app.license_popup_scroll_offset.min(max_offset);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" License ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let scroll = (app.license_popup_scroll_offset as u16, 0);
    let para = Paragraph::new(LICENSE_TEXT)
        .style(popup_style)
        .wrap(Wrap { trim: true })
        .scroll(scroll);
    f.render_widget(para, content_area);

    let hint = Paragraph::new("j/k or arrows: scroll | Esc / Enter / q: close")
        .style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn draw_whois_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(72).max(40);
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
        .constraints([Constraint::Min(2), Constraint::Length(1)])
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

    let text = if app.whois_lines.is_empty() {
        "(no data)".to_string()
    } else {
        app.whois_lines.join("\n")
    };
    let para = Paragraph::new(text)
        .style(popup_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let hint = Paragraph::new("Esc / Enter / q to close").style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
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
