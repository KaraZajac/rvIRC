//! UI: layout, status bar, message area, input bar, channel/user panes.

mod layout;

use crate::app::{App, MessageKind, MessageLine, Mode, PanelFocus, UserAction};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use ratatui_image::{Resize, StatefulImage};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CHANNELS_PANE_WIDTH: u16 = 22;
const USERS_PANE_WIDTH: u16 = 18;

/// Deterministic color for a nick (same nick → same color). Uses a hash of the nick.
/// Color for message source (nick or ***). System sources use dim gray.
/// Standard-replies: [FAIL] red, [WARN] yellow, [NOTE] dim.
fn source_color(source: &str) -> Color {
    if source == "***" || source.is_empty() {
        Color::DarkGray
    } else if source == "[FAIL]" {
        Color::Red
    } else if source == "[WARN]" {
        Color::Yellow
    } else if source == "[NOTE]" {
        Color::DarkGray
    } else {
        nick_color(source)
    }
}

fn nick_color(nick: &str) -> Color {
    const PALETTE: [Color; 16] = [
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::Rgb(255, 165, 0),   // orange
        Color::Rgb(147, 112, 219), // medium purple
        Color::Rgb(0, 206, 209),   // dark turquoise
        Color::Rgb(255, 105, 180), // hot pink
    ];
    let mut h: u64 = 0;
    for b in nick.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u64);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

/// Width in terminal cells of the input bar content (prompt + input + cursor if shown).
fn input_content_width(app: &App, show_cursor: bool) -> usize {
    let prompt = if app.mode == Mode::Command { ":" } else { "" };
    let cursor = app.input.floor_char_boundary(app.input_cursor.min(app.input.len()));
    let before = &app.input[..cursor];
    let after = app.input.get(cursor..).unwrap_or("");
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

    let left_visible = app.channel_panel_visible || app.messages_panel_visible;
    let right_visible = app.user_panel_visible || app.friends_panel_visible;
    let (center, left_opt, right_opt) = layout::center_with_side_panes(
        main_area,
        left_visible.then_some(CHANNELS_PANE_WIDTH),
        right_visible.then_some(USERS_PANE_WIDTH),
    );

    if let Some(left) = left_opt {
        let (channels_rect, messages_rect) = layout::split_vertical_by_visibility(left, app.channel_panel_visible, app.messages_panel_visible);
        if let Some(r) = channels_rect {
            draw_channels_pane(f, r, app);
        }
        if let Some(r) = messages_rect {
            draw_messages_pane(f, r, app);
        }
    }
    draw_message_area(f, center, app);
    if let Some(right) = right_opt {
        let (users_rect, friends_rect) = layout::split_vertical_by_visibility(right, app.user_panel_visible, app.friends_panel_visible);
        if let Some(r) = users_rect {
            draw_users_pane(f, r, app);
        }
        if let Some(r) = friends_rect {
            draw_friends_pane(f, r, app);
        }
    }

    draw_input_bar(f, input_area, app);
    draw_status_bar(f, status_area, app);

    if app.user_action_menu {
        if let Some(ref nick) = app.selected_user() {
            draw_user_action_menu(f, app, nick);
        }
    }

    if app.search_popup_visible {
        draw_search_popup(f, area, app);
    }
    if app.highlight_popup_visible {
        draw_highlight_popup(f, area, app);
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

    if app.ban_popup_visible {
        draw_ban_popup(f, area, app);
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

    if app.transfer_progress_visible {
        draw_transfer_progress_popup(f, area, app);
    }

    if app.file_browser_visible {
        draw_file_browser_popup(f, area, app);
    }
    if app.away_popup_visible {
        draw_away_popup(f, area, app);
    }
}

pub const IMAGE_DISPLAY_HEIGHT: u16 = 20;

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

pub fn message_wrapped_height(
    m: &MessageLine,
    _current_nick: Option<&str>,
    width: u16,
    reactions: &std::collections::HashMap<String, Vec<(String, String)>>,
) -> u16 {
    if width == 0 {
        return 2; // header + at least 1 message line
    }
    // Header: "nick | HH:mm" = 1 line. Message wraps below.
    let stripped = crate::format::strip_for_display_width(&m.text);
    let w = width as usize;
    let msg_width = stripped.width();
    let msg_lines = ((msg_width + w - 1) / w).max(1);
    let mut h = 1 + msg_lines as u16;
    if let Some(ref msgid) = m.msgid {
        if reactions.get(msgid).map_or(false, |v| !v.is_empty()) {
            h += 1; // draft/react line
        }
    }
    h
}

fn draw_message_area(f: &mut Frame, area: Rect, app: &mut App) {
    let server = app.current_server.as_deref().unwrap_or("");
    let target = app.current_channel.as_deref().unwrap_or("*server*");
    let target_key = crate::app::msg_key(server, target);
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

    // Reserve 1 row at bottom for typing indicator when present
    let typing_nicks = app.typing_nicks_for_target(server, target);
    let typing_reserved = if typing_nicks.is_empty() { 0 } else { 1 };
    let message_height = content_height.saturating_sub(typing_reserved);

    let messages: Vec<MessageLine> = app
        .current_messages()
        .iter()
        .filter(|m| !app.is_muted(&target_key, &m.source))
        .cloned()
        .collect();

    let nick_ref = app.current_nickname.as_deref();
    let mut item_heights: Vec<u16> = Vec::with_capacity(messages.len());
    for m in &messages {
        let text_h = message_wrapped_height(m, nick_ref, inner.width, &app.reactions);
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
        let mut remaining = message_height;
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
        if visible_end > 0 && visible_start == 0 && remaining >= message_height {
            visible_start = visible_end - 1;
        }
    }

    let visible_rows: usize = item_heights[visible_start..visible_end]
        .iter()
        .map(|h| *h as usize)
        .sum();
    let top_pad = message_height.saturating_sub(visible_rows) as u16;
    let mut cur_y = content_y + top_pad;
    let max_y = content_y + content_height as u16;
    let nick = app.current_nickname.as_deref().map(|s| s.to_string());
    let reply_numbers = if app.reply_select_mode {
        app.reply_select_numbers()
    } else {
        std::collections::HashMap::new()
    };

    for i in visible_start..visible_end {
        if cur_y >= max_y {
            break;
        }
        let m = &messages[i];
        let reply_num = m.msgid.as_ref().and_then(|id| reply_numbers.get(id).copied());
        let elapsed_ms = std::time::Instant::now().duration_since(app.created_at).as_millis() as u64;
        let text_h = message_wrapped_height(m, nick.as_deref(), inner.width, &app.reactions);
        let avail_text_h = text_h.min(max_y.saturating_sub(cur_y));
        let content = format_message_line_wrapped(m, nick.as_deref(), &app.highlight_words, &app.reactions, inner.width, elapsed_ms, reply_num);
        let text_rect = Rect { x: inner.x, y: cur_y, width: inner.width, height: avail_text_h };
        f.render_widget(
            Paragraph::new(content).wrap(Wrap { trim: true }),
            text_rect,
        );
        cur_y += avail_text_h;

        if let Some(img_id) = m.image_id {
            if let Some(inline) = app.inline_images.get_mut(&img_id) {
                inline.advance_frame();
                let protocol = inline.protocol_mut();
                let img_h = IMAGE_DISPLAY_HEIGHT.min(max_y.saturating_sub(cur_y));
                if img_h > 0 {
                    let img_rect = Rect { x: inner.x, y: cur_y, width: inner.width, height: img_h };
                    let image_widget = StatefulImage::default().resize(Resize::Scale(None));
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

    // IRCv3 typing indicator at bottom of message area (reserved row)
    if !typing_nicks.is_empty() {
        let msg = match typing_nicks.len() {
            1 => format!("{} is typing...", typing_nicks[0]),
            2 => format!("{} and {} are typing...", typing_nicks[0], typing_nicks[1]),
            n => format!("{} and {} others are typing...", typing_nicks[0], n - 1),
        };
        let style = Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM);
        let p = Paragraph::new(Line::from(Span::styled(msg, style)));
        let typing_y = content_y + content_height as u16 - 1;
        let r = Rect { x: inner.x, y: typing_y, width: inner.width, height: 1 };
        f.render_widget(p, r);
    }
}

/// Format a message line: header "nick | HH:mm" then message on following lines.
/// Nick gets a deterministic color. Parses IRC formatting (bold, italic, etc.) for the message.
/// Appends draft/react reactions if present.
/// reply_num: when in reply-select mode, 1-9 or 10 (displays as "0") to show which key selects this message.
fn format_message_line_wrapped(
    m: &MessageLine,
    current_nick: Option<&str>,
    highlight_words: &[String],
    reactions: &std::collections::HashMap<String, Vec<(String, String)>>,
    width: u16,
    elapsed_ms: u64,
    reply_num: Option<u8>,
) -> Text<'static> {
    let num_prefix = reply_num.map(|n| {
        let s = match n {
            1 => " 1",
            2 => " 2",
            3 => " 3",
            4 => " 4",
            5 => " 5",
            6 => " 6",
            7 => " 7",
            8 => " 8",
            9 => " 9",
            10 => " 0",
            _ => "  ",
        };
        Span::styled(s, Style::default().fg(Color::Cyan))
    }).unwrap_or_else(|| Span::raw(""));
    let time_str = m
        .timestamp
        .as_ref()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string());
    let header_style = source_color(&m.source);
    let mention = current_nick.map_or(false, |nick| {
        !nick.is_empty() && m.text.to_lowercase().contains(&nick.to_lowercase())
    });
    let account_prefix = m.account.as_ref().map(|a| format!("[{}] ", a)).unwrap_or_default();
    let bot_prefix = if m.is_bot_sender { "[bot] " } else { "" };
    let reply_indicator = m.reply_to_msgid.as_ref().map(|_| " ↷").unwrap_or_default();
    let mut header_spans = vec![num_prefix];
    if reply_num.is_some() {
        header_spans.push(Span::raw(" "));
    }
    header_spans.extend(vec![
        Span::styled(account_prefix, Style::default().add_modifier(Modifier::DIM)),
        Span::styled(bot_prefix, Style::default().fg(Color::Cyan)),
        Span::styled(m.source.clone(), header_style),
        Span::raw(" | "),
        Span::styled(time_str, Style::default().add_modifier(Modifier::DIM)),
        Span::styled(reply_indicator, Style::default().add_modifier(Modifier::DIM)),
    ]);
    let header_line = Line::from(header_spans);
    // Message body: IRC formatting, highlights, rainbow, etc.
    let mut msg_spans = crate::format::parse_message_with_rainbow(&m.text, elapsed_ms);
    let mut highlight_words = highlight_words.to_vec();
    if mention {
        if let Some(nick) = current_nick {
            highlight_words.push(nick.to_string());
        }
    }
    msg_spans = crate::format::apply_highlights_to_spans(msg_spans, &highlight_words);
    // CTCP ACTION (/me): render body in italics
    if m.kind == MessageKind::Action {
        msg_spans = msg_spans.into_iter()
            .map(|s| Span::styled(s.content, s.style.add_modifier(Modifier::ITALIC)))
            .collect();
    }
    let w = width as usize;
    let msg_lines = crate::format::wrap_spans(&msg_spans, w);
    let mut all_lines = vec![header_line];
    for line in msg_lines {
        all_lines.push(line);
    }
    // draft/react: show reactions under the message
    if let Some(ref msgid) = m.msgid {
        if let Some(reacts) = reactions.get(msgid) {
            if !reacts.is_empty() {
                let react_str: String = reacts.iter()
                    .map(|(nick, emoji)| format!(" {} {}", emoji, nick))
                    .collect::<Vec<_>>()
                    .join("");
                all_lines.push(Line::from(Span::styled(
                    format!("   ↷ {}", react_str.trim_start()),
                    Style::default().add_modifier(Modifier::DIM).fg(Color::Cyan),
                )));
            }
        }
    }
    Text::from(all_lines)
}

fn draw_input_bar(f: &mut Frame, area: Rect, app: &App) {
    let prompt = match app.mode {
        Mode::Command => ":",
        _ => "",
    };
    let show_cursor = app.mode == Mode::Insert || app.mode == Mode::Command;
    let line = if show_cursor {
        let elapsed_ms = std::time::Instant::now().duration_since(app.created_at).as_millis();
        let blink_on = (elapsed_ms / 530) % 2 == 0;
        let cursor_char = if blink_on { "|" } else { " " };
        let len = app.input.len();
        let cur = app.input.floor_char_boundary(app.input_cursor.min(len));
        let (raw_sel_lo, raw_sel_hi) = app.input_selection
            .map(|(a, b)| (a.min(b).min(len), a.max(b).min(len)))
            .unwrap_or((cur, cur));
        let sel_lo = app.input.floor_char_boundary(raw_sel_lo);
        let sel_hi = app.input.floor_char_boundary(raw_sel_hi);
        let sel = Style::default().add_modifier(Modifier::REVERSED);
        let mut spans: Vec<Span> = vec![Span::raw(prompt)];
        if cur > 0 {
            if cur <= sel_lo {
                spans.push(Span::raw(&app.input[0..cur]));
            } else {
                if sel_lo > 0 {
                    spans.push(Span::raw(&app.input[0..sel_lo]));
                }
                spans.push(Span::styled(&app.input[sel_lo..cur], sel));
            }
        }
        spans.push(Span::styled(cursor_char, Style::default()));
        if cur < len {
            if sel_hi <= cur {
                spans.push(Span::raw(&app.input[cur..len]));
            } else if cur < sel_lo {
                spans.push(Span::raw(&app.input[cur..sel_lo]));
                spans.push(Span::styled(&app.input[sel_lo..sel_hi.min(len)], sel));
                if sel_hi < len {
                    spans.push(Span::raw(&app.input[sel_hi..len]));
                }
            } else {
                spans.push(Span::styled(&app.input[cur..sel_hi.min(len)], sel));
                if sel_hi < len {
                    spans.push(Span::raw(&app.input[sel_hi..len]));
                }
            }
        }
        Line::from(spans)
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

fn target_display_label_for_entry(server: &str, target: &str) -> String {
    if target == "*server*" {
        server.to_string()
    } else {
        target.to_string()
    }
}

fn draw_channels_pane(f: &mut Frame, area: Rect, app: &App) {
    use crate::app::msg_key;
    let show_selector = app.panel_focus == PanelFocus::Channels;
    let channels_list = app.channels_list();
    let list_len = channels_list.len();
    let items: Vec<ListItem> = channels_list
        .iter()
        .enumerate()
        .map(|(i, (s, t))| {
            let label = target_display_label_for_entry(s, t);
            let key = msg_key(s, t);
            let secure = app.secure_sessions.contains_key(&key);
            let num = if show_selector {
                let n = i + 1;
                if n <= 9 { format!("{}", n) } else if n == 10 { "0".to_string() } else { String::new() }
            } else {
                String::new()
            };
            let prefix = if show_selector && i == app.channel_index {
                if t == "*server*" {
                    format!(">{} ", num)
                } else {
                    format!(">{}  ", num)
                }
            } else if show_selector && !num.is_empty() {
                if t == "*server*" {
                    format!(" {} ", num)
                } else {
                    format!(" {}  ", num)
                }
            } else if t == "*server*" {
                "  ".to_string()
            } else {
                "    ".to_string()
            };
            let style = if app.unread_mentions.contains(&key) {
                Style::default().fg(Color::Red)
            } else if app.unread_targets.contains(&key) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            if secure {
                let verified = app.known_keys.is_verified(t, s);
                let mut spans = vec![
                    Span::raw(prefix),
                    Span::styled("\u{1F512}", Style::default().fg(Color::Green)),
                ];
                if verified {
                    spans.push(Span::styled("\u{2714}", Style::default().fg(Color::Green)));
                }
                spans.push(Span::styled(format!("{}  ", label), style));
                ListItem::new(Line::from(spans))
            } else {
                let line = Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(format!("{}  ", label), style),
                ]);
                ListItem::new(line)
            }
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Servers & Channels "))
        .style(Style::default());
    let visible = area.height.saturating_sub(2) as usize;
    let list_len = list_len;
    let offset = if list_len <= visible || visible == 0 {
        0
    } else {
        (app.channel_index + 1)
            .saturating_sub(visible)
            .min(list_len.saturating_sub(visible))
            .max(0)
    };
    let mut state = ListState::default()
        .with_selected(Some(app.channel_index))
        .with_offset(offset);
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_messages_pane(f: &mut Frame, area: Rect, app: &App) {
    use crate::app::msg_key;
    let show_selector = app.panel_focus == PanelFocus::Messages;
    let messages_list = app.messages_list();
    let list_len = messages_list.len();
    let items: Vec<ListItem> = messages_list
        .iter()
        .enumerate()
        .map(|(i, (s, nick))| {
            let key = msg_key(s, nick);
            let secure = app.secure_sessions.contains_key(&key);
            let num = if show_selector {
                let n = i + 1;
                if n <= 9 { format!("{}", n) } else if n == 10 { "0".to_string() } else { String::new() }
            } else {
                String::new()
            };
            let prefix = if show_selector && i == app.messages_index {
                format!(">{} ", num)
            } else if show_selector && !num.is_empty() {
                format!(" {} ", num)
            } else {
                "  ".to_string()
            };
            let style = if app.unread_mentions.contains(&key) {
                Style::default().fg(Color::Red)
            } else if app.unread_targets.contains(&key) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            if secure {
                let verified = app.known_keys.is_verified(nick, s);
                let mut spans = vec![
                    Span::raw(prefix),
                    Span::styled("\u{1F512}", Style::default().fg(Color::Green)),
                ];
                if verified {
                    spans.push(Span::styled("\u{2714}", Style::default().fg(Color::Green)));
                }
                spans.push(Span::styled(format!("{}  ", nick), style));
                ListItem::new(Line::from(spans))
            } else {
                let line = Line::from(vec![
                    Span::raw(prefix),
                    Span::styled(format!("{}  ", nick), style),
                ]);
                ListItem::new(line)
            }
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Messages "))
        .style(Style::default());
    let visible = area.height.saturating_sub(2) as usize;
    let offset = if list_len <= visible || visible == 0 {
        0
    } else {
        (app.messages_index + 1)
            .saturating_sub(visible)
            .min(list_len.saturating_sub(visible))
            .max(0)
    };
    let mut state = ListState::default()
        .with_selected(Some(app.messages_index))
        .with_offset(offset);
    f.render_stateful_widget(list, area, &mut state);
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

/// Strip channel prefixes (~&@%+!.) from a user list entry to get the nick.
fn user_list_nick(entry: &str) -> &str {
    entry.trim_start_matches(|c: char| matches!(c, '~' | '&' | '@' | '%' | '+' | '!' | '.'))
}

fn draw_users_pane(f: &mut Frame, area: Rect, app: &App) {
    let show_selector = app.panel_focus == PanelFocus::Users;
    let server = app.current_server.as_deref().unwrap_or("");
    let filtered = app.filtered_user_list();
    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, u)| {
            let prefix = if show_selector && i == app.user_index {
                "> "
            } else {
                "  "
            };
            let role_style = user_prefix_style(u);
            let nick = user_list_nick(u);
            let account_suffix = app
                .account_per_nick
                .get(&(server.to_string(), nick.to_lowercase()))
                .and_then(|a| a.as_ref())
                .map(|a| format!(" ({})", a))
                .unwrap_or_default();
            let userhost_suffix = app
                .userhost_per_nick
                .get(&(server.to_string(), nick.to_lowercase()))
                .map(|h| format!(" [{}]", h))
                .unwrap_or_default();
            let bot_suffix = if app.bot_per_nick.contains(&(server.to_string(), nick.to_lowercase())) {
                " [bot]"
            } else {
                ""
            };
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(u.as_str(), role_style),
                Span::styled(account_suffix, Style::default().add_modifier(Modifier::DIM)),
                Span::styled(userhost_suffix, Style::default().add_modifier(Modifier::DIM)),
                Span::styled(bot_suffix, Style::default().fg(Color::Cyan)),
                Span::raw("  "),
            ]);
            ListItem::new(line)
        })
        .collect();
    let title = if app.user_list_filter.is_empty() {
        " Users ".to_string()
    } else {
        format!(" Users [f filter: {}] ", app.user_list_filter)
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .style(Style::default());
    let visible = area.height.saturating_sub(2) as usize;
    let filtered_len = filtered.len();
    let offset = if filtered_len <= visible || visible == 0 {
        0
    } else {
        (app.user_index + 1).saturating_sub(visible).min(filtered_len.saturating_sub(visible)).max(0)
    };
    let mut state = ListState::default().with_selected(Some(app.user_index)).with_offset(offset);
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_friends_pane(f: &mut Frame, area: Rect, app: &App) {
    let show_selector = app.panel_focus == PanelFocus::Friends;
    let visible = app.visible_friends();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(i, nick)| {
            let prefix = if show_selector && i == app.friends_index {
                "> "
            } else {
                "  "
            };
            let (online, away) = app.friend_status(nick);
            let nick_color = if !online {
                Color::Red
            } else if away {
                Color::Yellow
            } else {
                Color::Green
            };
            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(nick, Style::default().fg(nick_color)),
                Span::raw("  "),
            ]);
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Friends "))
        .style(Style::default());
    let visible_height = area.height.saturating_sub(2) as usize;
    let offset = if visible.len() <= visible_height {
        0
    } else {
        (app.friends_index + 1)
            .saturating_sub(visible_height)
            .min(visible.len() - visible_height)
            .max(0)
    };
    let mut state = ListState::default()
        .with_selected(Some(app.friends_index))
        .with_offset(offset);
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
    f.render_widget(Clear, menu_rect);
    let popup_style = popup_overlay_style();
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
                .title(format!(" {} ", nick))
                .style(popup_style),
        )
        .style(popup_style);
    let visible = menu_rect.height.saturating_sub(2) as usize;
    let len = actions.len();
    let offset = if len <= visible {
        0
    } else {
        (app.user_action_index + 1).saturating_sub(visible).min(len.saturating_sub(visible)).max(0)
    };
    let mut state = ListState::default()
        .with_selected(Some(app.user_action_index))
        .with_offset(offset);
    f.render_stateful_widget(list, menu_rect, &mut state);
}

fn draw_search_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(70).max(40);
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

    let list_items: Vec<ListItem> = if app.search_results.is_empty() {
        let msg = if app.search_filter.is_empty() {
            "No messages in buffer"
        } else {
            "No matches"
        };
        vec![ListItem::new(format!("  {}  ", msg))]
    } else {
        app.search_results
            .iter()
            .enumerate()
            .map(|(i, (idx, preview))| {
                let line = if i == app.search_selected_index {
                    format!("> [{}] {}  ", idx + 1, preview)
                } else {
                    format!("  [{}] {}  ", idx + 1, preview)
                };
                ListItem::new(line)
            })
            .collect()
    };

    let filter_display = if app.search_filter.is_empty() {
        "Filter: (type to search)".to_string()
    } else {
        format!("Filter: {}", app.search_filter)
    };
    let mode_hint = if app.search_scroll_mode {
        "j/k or arrows: move | Enter: jump to message | Esc: back to search"
    } else {
        "Type to search | Enter: browse list | Esc: close"
    };

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Search ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let filter_para = Paragraph::new(filter_display).style(popup_style);
    f.render_widget(filter_para, chunks[0]);
    let hint_para = Paragraph::new(mode_hint).style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint_para, chunks[1]);

    let list = List::new(list_items).style(popup_style);
    let list_area = chunks[2];
    let visible = list_area.height as usize;
    let len = app.search_results.len();
    let offset = if len <= visible || visible == 0 {
        0
    } else {
        (app.search_selected_index + 1)
            .saturating_sub(visible)
            .min(len.saturating_sub(visible))
            .max(0)
    };
    let mut list_state = ListState::default()
        .with_selected(if app.search_results.is_empty() {
            None
        } else {
            Some(app.search_selected_index)
        })
        .with_offset(offset);
    f.render_stateful_widget(list, list_area, &mut list_state);
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
    let show_server = app.channel_list_super;
    let list_items: Vec<ListItem> = if filtered.is_empty() {
        let msg = if app.server_channel_list.is_empty()
            && app.channel_list_pending_servers.is_empty()
        {
            "Loading..."
        } else if app.server_channel_list.is_empty() {
            "Fetching from all servers..."
        } else {
            "No channels match filter"
        };
        vec![ListItem::new(format!("  {}  ", msg))]
    } else {
        filtered
            .iter()
            .enumerate()
            .map(|(i, (server, channel, count))| {
                let label = if show_server {
                    match count {
                        Some(n) => format!("{} {} ({})", server, channel, n),
                        None => format!("{} {}", server, channel),
                    }
                } else {
                    match count {
                        Some(n) => format!("{} ({})", channel, n),
                        None => channel.clone(),
                    }
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
    let title = if app.channel_list_super {
        " Channel list (all servers) "
    } else {
        " Channel list "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
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

fn draw_away_popup(f: &mut Frame, area: Rect, app: &App) {
    let reason = app.away_message.as_deref().unwrap_or("");
    let popup_width = 40;
    let popup_height = 5;
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
    let yellow_style = Style::default().fg(Color::Yellow);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" AWAY ")
        .style(yellow_style);
    f.render_widget(block, popup_rect);

    let text = if reason.is_empty() {
        "Away".to_string()
    } else {
        reason.to_string()
    };
    let para = Paragraph::new(text)
        .style(yellow_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let hint = Paragraph::new("Any key to cancel").style(yellow_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn draw_highlight_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(44).max(28);
    let popup_height = (area.height * 3 / 4).min(20).max(10);
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

    let add_display = if app.highlight_input.is_empty() {
        "Add: (type and Enter)".to_string()
    } else {
        format!("Add: {}", app.highlight_input)
    };
    let list_items: Vec<ListItem> = if app.highlight_words.is_empty() {
        vec![ListItem::new("  (no highlight words)  ")]
    } else {
        app.highlight_words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let line = if i == app.highlight_selected_index {
                    format!("> {}  ", w)
                } else {
                    format!("  {}  ", w)
                };
                ListItem::new(line)
            })
            .collect()
    };

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Highlights ")
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let add_para = Paragraph::new(add_display).style(popup_style);
    f.render_widget(add_para, chunks[0]);
    let hint = Paragraph::new("j/k: move | d: remove | Enter: add word | Esc: close").style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);

    let list = List::new(list_items).style(popup_style);
    let list_area = chunks[2];
    let visible = list_area.height as usize;
    let len = app.highlight_words.len();
    let offset = if len <= visible || visible == 0 {
        0
    } else {
        (app.highlight_selected_index + 1)
            .saturating_sub(visible)
            .min(len.saturating_sub(visible))
            .max(0)
    };
    let mut list_state = ListState::default()
        .with_selected(if app.highlight_words.is_empty() {
            None
        } else {
            Some(app.highlight_selected_index)
        })
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

fn draw_ban_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = (area.width * 3 / 4).min(80).max(40);
    let popup_height = (area.height * 3 / 4).min(24).max(8);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect { x, y, width: popup_width, height: popup_height };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    let title = if app.ban_popup_channel.is_empty() {
        " Ban List ".to_string()
    } else {
        format!(" Ban List: {} ", app.ban_popup_channel)
    };

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let inner_height = chunks[0].height as usize;
    let entries = &app.ban_popup_entries;
    let scroll = app.ban_popup_scroll.min(entries.len().saturating_sub(1));
    let visible: Vec<String> = if entries.is_empty() {
        vec!["(no bans)".to_string()]
    } else {
        entries.iter()
            .enumerate()
            .skip(scroll)
            .take(inner_height)
            .map(|(i, mask)| {
                // Pretty-print $a:account extbans.
                let display = if let Some(account) = mask.strip_prefix("$a:") {
                    format!("account:{}", account)
                } else if mask == "$a" {
                    "account:(any authenticated)".to_string()
                } else {
                    mask.clone()
                };
                format!("{:3}. {}", i + 1, display)
            })
            .collect()
    };
    let text = visible.join("\n");
    let para = Paragraph::new(text)
        .style(popup_style)
        .wrap(Wrap { trim: false });
    f.render_widget(para, chunks[0]);

    let hint_text = if entries.len() > inner_height {
        format!("j/k scroll · {} entries · Esc/q to close", entries.len())
    } else {
        "Esc / q to close".to_string()
    };
    let hint = Paragraph::new(hint_text).style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(hint, chunks[1]);
}

fn action_label(a: &UserAction) -> &'static str {
    match a {
        UserAction::Dm => "Direct message",
        UserAction::Kick => "Kick",
        UserAction::Ban => "Ban",
        UserAction::Unban => "Unban",
        UserAction::Op => "Op",
        UserAction::Deop => "Deop",
        UserAction::Voice => "Voice",
        UserAction::Devoice => "Devoice",
        UserAction::Halfop => "Halfop",
        UserAction::Dehalfop => "Dehalfop",
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

fn draw_transfer_progress_popup(f: &mut Frame, area: Rect, app: &App) {
    let popup_width = 50;
    let popup_height = 8;
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_rect = Rect { x, y, width: popup_width, height: popup_height };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(2), Constraint::Length(2), Constraint::Length(1)])
        .margin(1)
        .split(popup_rect);

    f.render_widget(Clear, popup_rect);
    let popup_style = popup_overlay_style();
    let title = if app.transfer_progress_is_send {
        " Sending "
    } else {
        " Receiving "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(popup_style);
    f.render_widget(block, popup_rect);

    let size_display = if app.transfer_progress_total >= 1_048_576 {
        format!("{:.1} / {:.1} MB",
            app.transfer_progress_bytes as f64 / 1_048_576.0,
            app.transfer_progress_total as f64 / 1_048_576.0)
    } else if app.transfer_progress_total >= 1024 {
        format!("{:.1} / {:.1} KB",
            app.transfer_progress_bytes as f64 / 1024.0,
            app.transfer_progress_total as f64 / 1024.0)
    } else {
        format!("{} / {} B", app.transfer_progress_bytes, app.transfer_progress_total)
    };

    let text = format!(
        "{} → {}\n{}\n",
        if app.transfer_progress_is_send { "To" } else { "From" },
        app.transfer_progress_nick,
        app.transfer_progress_filename
    );
    let para = Paragraph::new(text)
        .style(popup_style)
        .wrap(Wrap { trim: true });
    f.render_widget(para, chunks[0]);

    let pct = if app.transfer_progress_total > 0 {
        ((app.transfer_progress_bytes as f64 / app.transfer_progress_total as f64) * 100.0) as u16
    } else {
        0
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Green))
        .percent(pct.min(100));
    f.render_widget(gauge, chunks[1]);

    let info = Paragraph::new(size_display)
        .style(popup_style.add_modifier(Modifier::DIM));
    f.render_widget(info, chunks[2]);
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
