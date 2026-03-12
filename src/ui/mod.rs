//! UI: layout, status bar, message area, input bar, channel/user panes.

mod layout;

use crate::app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::StatefulImage;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

    if app.file_receive_popup_visible {
        draw_file_receive_popup(f, area, app);
    }

    if app.secure_accept_popup_visible {
        draw_secure_accept_popup(f, area, app);
    }

    if app.file_browser_visible {
        draw_file_browser_popup(f, area, app);
    }
}

const IMAGE_DISPLAY_HEIGHT: u16 = 12;

/// Break a string at character boundaries so no line exceeds max_width display columns.
fn wrap_str_at_width(s: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![s.to_string()];
    }
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut w: usize = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(1);
        if w + cw > max_width && !current.is_empty() {
            segments.push(std::mem::take(&mut current));
            w = 0;
        }
        current.push(ch);
        w += cw;
    }
    if !current.is_empty() {
        segments.push(current);
    }
    if segments.is_empty() && !s.is_empty() {
        segments.push(s.to_string());
    }
    segments
}

fn message_wrapped_height(m: &MessageLine, _current_nick: Option<&str>, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let prefix = match m.kind {
        MessageKind::Privmsg => format!("<{}> ", m.source),
        MessageKind::Notice => format!("[{}] ", m.source),
        MessageKind::Action => format!("* {} ", m.source),
        MessageKind::Join => format!("*** {} ", m.source),
        MessageKind::Part | MessageKind::Quit => format!("*** {} ", m.source),
        MessageKind::Nick => format!("*** {} ", m.source),
        MessageKind::Mode => "*** ".to_string(),
        MessageKind::Other => format!("{} ", m.source),
    };
    let full = format!("{}{}", prefix, m.text);
    let w = width as usize;
    let display_width = full.width();
    ((display_width + w - 1) / w).max(1) as u16
}

fn draw_message_area(f: &mut Frame, area: Rect, app: &mut App) {
    let target_key = app.current_channel.as_deref().unwrap_or("*server*").to_string();
    let title = app.current_target_title();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut header_height: u16 = 0;
    if let Some(topic) = app.current_topic() {
        let topic_text = format!(" Topic: {} ", topic);
        let segments = wrap_str_at_width(&topic_text, inner.width as usize);
        let topic_h = segments.len().min(inner.height.saturating_sub(header_height) as usize) as u16;
        let style = Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM);
        let lines: Vec<Line> = segments
            .iter()
            .take(topic_h as usize)
            .map(|s| Line::from(Span::styled(s.clone(), style)))
            .collect();
        let r = Rect { x: inner.x, y: inner.y + header_height, width: inner.width, height: topic_h };
        f.render_widget(Paragraph::new(Text::from(lines)), r);
        header_height += topic_h;
    }
    if let Some(modes) = app.current_modes() {
        let modes_text = format!(" Modes: {} ", modes);
        let segments = wrap_str_at_width(&modes_text, inner.width as usize);
        let modes_h = segments.len().min(inner.height.saturating_sub(header_height) as usize) as u16;
        let style = Style::default().fg(Color::Green).add_modifier(Modifier::DIM);
        let lines: Vec<Line> = segments
            .iter()
            .take(modes_h as usize)
            .map(|s| Line::from(Span::styled(s.clone(), style)))
            .collect();
        let r = Rect { x: inner.x, y: inner.y + header_height, width: inner.width, height: modes_h };
        f.render_widget(Paragraph::new(Text::from(lines)), r);
        header_height += modes_h;
    }

    let content_y = inner.y + header_height;
    let content_height = inner.height.saturating_sub(header_height) as usize;
    if content_height == 0 {
        return;
    }

    let messages: Vec<MessageLine> = app
        .current_messages()
        .iter()
        .filter(|m| !app.is_muted(&target_key, &m.source))
        .cloned()
        .collect();

    let nick_ref = app.current_nickname.as_deref();
    let mut item_heights: Vec<u16> = Vec::with_capacity(messages.len());
    for m in &messages {
        let text_h = message_wrapped_height(m, nick_ref, inner.width);
        let h = match m.image_id {
            Some(id) if app.inline_images.contains_key(&id) => text_h + IMAGE_DISPLAY_HEIGHT,
            Some(_) => text_h + 1,
            None => text_h,
        };
        item_heights.push(h);
    }

    let total_rows: usize = item_heights.iter().map(|h| *h as usize).sum();
    let scroll = app.message_scroll_offset;

    let rows_from_bottom = scroll;
    let bottom_skip: usize = rows_from_bottom.min(total_rows);

    let mut visible_end = messages.len();
    {
        let mut acc: usize = 0;
        for i in (0..messages.len()).rev() {
            acc += item_heights[i] as usize;
            if acc > bottom_skip {
                visible_end = i + 1;
                break;
            }
            if acc == bottom_skip {
                visible_end = i;
                break;
            }
        }
        if acc < bottom_skip {
            visible_end = 0;
        }
    }

    let mut visible_start = 0;
    {
        let mut remaining = content_height;
        let mut i = visible_end;
        while i > 0 {
            i -= 1;
            let h = item_heights[i] as usize;
            if h > remaining {
                if visible_start == 0 && i + 1 == visible_end {
                    visible_start = i;
                }
                break;
            }
            remaining -= h;
            visible_start = i;
        }
        if visible_end > 0 && visible_start == 0 && remaining >= content_height {
            visible_start = visible_end - 1;
        }
    }

    let visible_rows: usize = item_heights[visible_start..visible_end]
        .iter()
        .map(|h| *h as usize)
        .sum();
    let top_pad = content_height.saturating_sub(visible_rows) as u16;
    let mut cur_y = content_y + top_pad;
    let max_y = content_y + content_height as u16;
    let nick = app.current_nickname.as_deref().map(|s| s.to_string());

    for i in visible_start..visible_end {
        if cur_y >= max_y {
            break;
        }
        let m = &messages[i];
        let text_h = message_wrapped_height(m, nick.as_deref(), inner.width);
        let avail_text_h = text_h.min(max_y.saturating_sub(cur_y));
        let content = format_message_line_wrapped(m, nick.as_deref(), inner.width);
        let text_rect = Rect { x: inner.x, y: cur_y, width: inner.width, height: avail_text_h };
        f.render_widget(
            Paragraph::new(content).wrap(Wrap { trim: true }),
            text_rect,
        );
        cur_y += avail_text_h;

        if let Some(img_id) = m.image_id {
            if let Some(protocol) = app.inline_images.get_mut(&img_id) {
                let img_h = IMAGE_DISPLAY_HEIGHT.min(max_y.saturating_sub(cur_y));
                if img_h > 0 {
                    let img_rect = Rect { x: inner.x, y: cur_y, width: inner.width, height: img_h };
                    let image_widget = StatefulImage::default();
                    f.render_stateful_widget(image_widget, img_rect, protocol);
                    cur_y += img_h;
                }
            } else if cur_y < max_y {
                let loading = Paragraph::new(Line::from(Span::styled(
                    "  [Loading image...]",
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                )));
                let lr = Rect { x: inner.x, y: cur_y, width: inner.width, height: 1 };
                f.render_widget(loading, lr);
                cur_y += 1;
            }
        }
    }
}

/// Format a message line with styling, pre-wrapped at character boundaries so long words/URLs wrap.
fn format_message_line_wrapped(
    m: &MessageLine,
    current_nick: Option<&str>,
    width: u16,
) -> Text<'static> {
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
    let full = format!("{}{}", prefix, m.text);
    let w = width as usize;
    let segments = wrap_str_at_width(&full, w);
    let default_style = Style::default();
    let lines: Vec<Line> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            if i == 0 && seg.len() >= prefix.len() && seg.starts_with(prefix.as_str()) {
                let rest = seg[prefix.len()..].to_string();
                Line::from(vec![
                    Span::styled(prefix.clone(), style),
                    Span::styled(rest, default_style),
                ])
            } else {
                Line::from(Span::styled(seg.clone(), default_style))
            }
        })
        .collect();
    Text::from(lines)
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
            let secure = app.secure_sessions.contains_key(t);
            let prefix = if show_selector && i == app.channel_index {
                "> "
            } else {
                "  "
            };
            let style = if app.unread_mentions.contains(t) {
                Style::default().fg(Color::Red)
            } else if app.unread_targets.contains(t) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            if secure {
                let server = app.current_server.as_deref().unwrap_or("unknown");
                let verified = app.known_keys.is_verified(t, server);
                let mut spans = vec![
                    Span::styled(prefix, style),
                    Span::styled("\u{1F512}", Style::default().fg(Color::Green)),
                ];
                if verified {
                    spans.push(Span::styled("\u{2714}", Style::default().fg(Color::Green)));
                }
                spans.push(Span::styled(format!("{}  ", label), style));
                let line = Line::from(spans);
                ListItem::new(line)
            } else {
                ListItem::new(Line::from(Span::styled(format!("{}{}  ", prefix, label), style)))
            }
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

fn draw_secure_accept_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = 60;
    let popup_height = if app.secure_accept_key_changed { 12 } else { 9 };
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect { x, y, width: popup_width, height: popup_height };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Secure Session Request ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::raw(format!(
            "{} wants to establish a secure session.",
            app.secure_accept_nick
        ))),
        Line::from(""),
    ];

    if app.secure_accept_key_changed {
        lines.push(Line::from(Span::styled(
            "WARNING: This user's identity key has CHANGED!",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "This could indicate a man-in-the-middle attack.",
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from("Accept? (y/n)"));

    let para = Paragraph::new(Text::from(lines))
        .style(popup_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let hint = Paragraph::new("y / Enter: Accept | n / Esc: Reject")
        .style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn draw_file_receive_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = 56;
    let popup_height = 9;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect { x, y, width: popup_width, height: popup_height };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" File Transfer ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let size_display = if app.file_receive_size >= 1_048_576 {
        format!("{:.1} MB", app.file_receive_size as f64 / 1_048_576.0)
    } else if app.file_receive_size >= 1024 {
        format!("{:.1} KB", app.file_receive_size as f64 / 1024.0)
    } else {
        format!("{} B", app.file_receive_size)
    };

    let text = format!(
        "{} wants to send you:\n  {} ({})\n\nAccept?",
        app.file_receive_nick, app.file_receive_filename, size_display
    );
    let para = Paragraph::new(text)
        .style(popup_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let hint = Paragraph::new("y / Enter: Accept | n / Esc: Reject")
        .style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn draw_file_browser_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(64).max(36);
    let popup_height = (area.height * 3 / 4).min(22).max(10);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect { x, y, width: popup_width, height: popup_height };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(popup_rect);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    use crate::app::FileBrowserMode;
    let title = match app.file_browser_mode {
        FileBrowserMode::ReceiveFile => " Choose Save Directory ",
        FileBrowserMode::SendFile => " Choose File to Send ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let path_str = app.file_browser_path.display().to_string();
    let path_para = Paragraph::new(path_str).style(popup_style.add_modifier(Modifier::BOLD));
    f.render_widget(path_para, chunks[0]);

    let list_items: Vec<ListItem> = if app.file_browser_entries.is_empty() {
        vec![ListItem::new("  (empty directory)")]
    } else {
        app.file_browser_entries
            .iter()
            .enumerate()
            .map(|(i, (name, is_dir))| {
                let prefix = if i == app.file_browser_selected_index { "> " } else { "  " };
                let suffix = if *is_dir { "/" } else { "" };
                let style = if *is_dir {
                    popup_style.fg(Color::Cyan)
                } else {
                    popup_style
                };
                ListItem::new(Line::from(Span::styled(format!("{}{}{}", prefix, name, suffix), style)))
            })
            .collect()
    };

    let list = List::new(list_items).style(popup_style);
    let list_area = chunks[1];
    let visible = list_area.height as usize;
    let len = app.file_browser_entries.len();
    let offset = if len <= visible || visible == 0 {
        0
    } else {
        (app.file_browser_selected_index + 1)
            .saturating_sub(visible)
            .min(len.saturating_sub(visible))
    };
    let mut list_state = ListState::default()
        .with_selected(Some(app.file_browser_selected_index))
        .with_offset(offset);
    f.render_stateful_widget(list, list_area, &mut list_state);

    let hint_text = match app.file_browser_mode {
        FileBrowserMode::ReceiveFile => "j/k: navigate | Enter: open dir | Backspace: up | s: save here | Esc: cancel",
        FileBrowserMode::SendFile => "j/k: navigate | Enter: open dir / select file | Backspace: up | Esc: cancel",
    };
    let hint = Paragraph::new(hint_text)
        .style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[2]);
}
