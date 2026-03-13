//! IRC message formatting: user-friendly input ↔ IRC control codes.
//! See https://modern.ircdocs.horse/formatting

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

// IRC control characters (from modern.ircdocs.horse)
const BOLD: char = '\x02';
const ITALIC: char = '\x1D';
const STRIKETHROUGH: char = '\x1E';
const _UNDERLINE: char = '\x1F';
const COLOR: char = '\x03';
const RESET: char = '\x0F';

/// Convert user-friendly syntax to IRC control codes before sending.
/// Supports: *italic*, **bold**, ***bold italic***, ~~strikethrough~~, ||spoiler||,
/// and :colorname: text :colorname: for colors.
pub fn format_outgoing(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        let rest = &text[i..];

        // ***bold italic***
        if rest.starts_with("***") {
            if let Some(end) = find_matching_delim(rest, "***") {
                out.push(BOLD);
                out.push(ITALIC);
                out.push_str(&format_outgoing(&rest[3..end]));
                out.push(ITALIC);
                out.push(BOLD);
                i += end + 3;
                continue;
            }
        }

        // **bold**
        if rest.starts_with("**") {
            if let Some(end) = find_matching_delim(rest, "**") {
                out.push(BOLD);
                out.push_str(&format_outgoing(&rest[2..end]));
                out.push(BOLD);
                i += end + 2;
                continue;
            }
        }

        // *italic* (single - not part of ** or ***)
        if rest.starts_with('*') && !rest.get(1..3).map_or(false, |s| s == "*") {
            if let Some(end) = find_matching_delim(rest, "*") {
                out.push(ITALIC);
                out.push_str(&format_outgoing(&rest[1..end]));
                out.push(ITALIC);
                i += end + 1;
                continue;
            }
        }

        // ~~strikethrough~~
        if rest.starts_with("~~") {
            if let Some(end) = find_matching_delim(rest, "~~") {
                out.push(STRIKETHROUGH);
                out.push_str(&rest[2..end]);
                out.push(STRIKETHROUGH);
                i += end + 2;
                continue;
            }
        }

        // ||spoiler|| (IRC: same fg/bg - we use grey 14)
        if rest.starts_with("||") {
            if let Some(end) = find_matching_delim(rest, "||") {
                out.push(COLOR);
                out.push_str("14,14"); // grey fg and bg
                out.push_str(&rest[2..end]);
                out.push(RESET);
                i += end + 2;
                continue;
            }
        }

        // :colorname: ... :colorname: or :normal: to reset
        if rest.starts_with(':') {
            if let Some((colorname, content_end)) = parse_color_zone(rest) {
                let zone_start = colorname.len() + 2;
                let content_len = content_end.saturating_sub(zone_start);
                if colorname == "normal" {
                    out.push(RESET);
                    out.push_str(&rest[zone_start..zone_start + content_len]);
                    out.push(RESET);
                    i += content_end;
                    continue;
                }
                if let Some(code) = color_name_to_irc_code(&colorname) {
                    out.push(COLOR);
                    out.push_str(&code.to_string());
                    out.push_str(&rest[zone_start..zone_start + content_len]);
                    out.push(RESET);
                    i += content_end;
                    continue;
                }
            }
        }

        // Plain char
        out.push_str(&rest[..rest.chars().next().map_or(0, |c| c.len_utf8())]);
        i += rest.chars().next().map_or(0, |c| c.len_utf8());
    }

    out
}

fn find_matching_delim(s: &str, delim: &str) -> Option<usize> {
    let len = delim.len();
    if s.len() < len * 2 {
        return None;
    }
    let mut i = len;
    while i + len <= s.len() {
        if &s[i..i + len] == delim {
            return Some(i);
        }
        // Skip nested same-length delims for * vs ** vs ***
        if delim == "*" && s[i..].starts_with("**") {
            i += 2; // skip **
            continue;
        }
        if delim == "**" && s[i..].starts_with('*') && !s[i + 1..].starts_with('*') {
            i += 1;
            continue;
        }
        i += s[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    None
}

fn parse_color_zone(s: &str) -> Option<(String, usize)> {
    if !s.starts_with(':') {
        return None;
    }
    let after_colon = &s[1..];
    let end_name = after_colon.find(':')?;
    let colorname = after_colon[..end_name].to_lowercase();
    let zone_start = end_name + 2; // skip :name:
    if zone_start >= s.len() {
        return Some((colorname, s.len()));
    }
    let rest = &s[zone_start..];
    // Find next :colorname: (any color) - that ends this zone
    let mut i = 0;
    while i < rest.len() {
        if rest.as_bytes().get(i) == Some(&b':') {
            let after = &rest[i + 1..];
            if let Some(end) = after.find(':') {
                let name = &after[..end];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return Some((colorname, zone_start + i));
                }
            }
        }
        i += rest[i..].chars().next().map_or(1, |c| c.len_utf8());
    }
    Some((colorname, s.len()))
}

fn color_name_to_irc_code(name: &str) -> Option<u8> {
    let map: HashMap<&str, u8> = [
        ("white", 0),
        ("black", 1),
        ("blue", 2),
        ("green", 3),
        ("red", 4),
        ("brown", 5),
        ("magenta", 6),
        ("orange", 7),
        ("yellow", 8),
        ("lightgreen", 9),
        ("light green", 9),
        ("cyan", 10),
        ("lightcyan", 11),
        ("light cyan", 11),
        ("lightblue", 12),
        ("light blue", 12),
        ("pink", 13),
        ("grey", 14),
        ("gray", 14),
        ("lightgrey", 15),
        ("light grey", 15),
        ("lightgray", 15),
        ("light gray", 15),
    ]
    .into_iter()
    .collect();
    map.get(name.to_lowercase().as_str()).copied()
}

/// Parse IRC control codes and produce styled spans for display.
pub fn parse_irc_formatting(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    let mut modifiers = Modifier::empty();
    let mut fg: Option<Color> = None;
    let mut bg: Option<Color> = None;
    let mut buf = String::new();

    let irc_colors: [(u8, Color); 16] = [
        (0, Color::White),
        (1, Color::Black),
        (2, Color::Blue),
        (3, Color::Green),
        (4, Color::Red),
        (5, Color::Rgb(165, 42, 42)), // brown
        (6, Color::Magenta),
        (7, Color::Rgb(255, 165, 0)), // orange
        (8, Color::Yellow),
        (9, Color::LightGreen),
        (10, Color::Cyan),
        (11, Color::LightCyan),
        (12, Color::LightBlue),
        (13, Color::Rgb(255, 192, 203)), // pink
        (14, Color::DarkGray),
        (15, Color::Gray),
    ];

    let bytes = text.as_bytes();

    while i < bytes.len() {
        let b = bytes[i];

        match b {
            0x02 => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                modifiers.toggle(Modifier::BOLD);
                i += 1;
            }
            0x1D => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                modifiers.toggle(Modifier::ITALIC);
                i += 1;
            }
            0x1E => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                modifiers.toggle(Modifier::CROSSED_OUT);
                i += 1;
            }
            0x1F => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                modifiers.toggle(Modifier::UNDERLINED);
                i += 1;
            }
            0x0F => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                modifiers = Modifier::empty();
                fg = None;
                bg = None;
                i += 1;
            }
            0x03 => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                let (adv, new_fg, new_bg) = parse_color_code(&bytes[i..], &irc_colors);
                i += adv;
                fg = new_fg;
                bg = new_bg;
            }
            0x04 => {
                flush_span(&mut buf, modifiers, fg, bg, &mut spans);
                let (adv, color) = parse_hex_color(&bytes[i..]);
                i += adv;
                fg = Some(color);
            }
            _ => {
                let ch = text[i..].chars().next().unwrap_or('\0');
                buf.push(ch);
                i += ch.len_utf8();
            }
        }
    }

    flush_span(&mut buf, modifiers, fg, bg, &mut spans);
    spans
}

fn flush_span(
    buf: &mut String,
    modifiers: Modifier,
    fg: Option<Color>,
    bg: Option<Color>,
    spans: &mut Vec<Span<'static>>,
) {
    if buf.is_empty() {
        return;
    }
    let mut style = Style::default();
    if modifiers.contains(Modifier::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if modifiers.contains(Modifier::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if modifiers.contains(Modifier::CROSSED_OUT) {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if modifiers.contains(Modifier::UNDERLINED) {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if let Some(c) = fg {
        style = style.fg(c);
    }
    if let Some(c) = bg {
        style = style.bg(c);
    }
    spans.push(Span::styled(std::mem::take(buf), style));
}

fn parse_color_code(bytes: &[u8], irc_colors: &[(u8, Color)]) -> (usize, Option<Color>, Option<Color>) {
    if bytes.len() < 2 {
        return (1, None, None);
    }
    let mut i = 1; // skip 0x03
    let mut fg: Option<Color> = None;
    let mut bg: Option<Color> = None;

    // Optional comma then digits
    let read_digit = |start: usize| -> (usize, Option<u8>) {
        let mut j = start;
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            let mut n = (bytes[j] - b'0') as u8;
            j += 1;
            if j < bytes.len() && bytes[j].is_ascii_digit() {
                n = n * 10 + (bytes[j] - b'0') as u8;
                j += 1;
            }
            return (j, Some(n));
        }
        (j, None)
    };

    let (j, fg_code) = read_digit(i);
    i = j;
    if let Some(code) = fg_code {
        fg = irc_colors.iter().find(|(c, _)| *c == code).map(|(_, col)| *col);
    }

    if i < bytes.len() && bytes[i] == b',' {
        i += 1;
        let (j, bg_code) = read_digit(i);
        i = j;
        if let Some(code) = bg_code {
            bg = irc_colors.iter().find(|(c, _)| *c == code).map(|(_, col)| *col);
        }
    }

    (i, fg, bg)
}

fn parse_hex_color(bytes: &[u8]) -> (usize, Color) {
    if bytes.len() < 7 {
        return (1, Color::White);
    }
    if bytes[1] != b'#' && !bytes[1].is_ascii_hexdigit() {
        return (1, Color::White);
    }
    let start = if bytes[1] == b'#' { 2 } else { 1 };
    if start + 6 > bytes.len() {
        return (1, Color::White);
    }
    let r = u8::from_str_radix(std::str::from_utf8(&bytes[start..start + 2]).unwrap_or("00"), 16).unwrap_or(0);
    let g = u8::from_str_radix(std::str::from_utf8(&bytes[start + 2..start + 4]).unwrap_or("00"), 16).unwrap_or(0);
    let b = u8::from_str_radix(std::str::from_utf8(&bytes[start + 4..start + 6]).unwrap_or("00"), 16).unwrap_or(0);
    (start + 6, Color::Rgb(r, g, b))
}

/// Strip IRC control codes for display-width calculation.
pub fn strip_irc_codes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        match bytes[i] {
            0x02 | 0x1D | 0x1E | 0x1F | 0x0F => i += 1,
            0x03 => {
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b',') {
                    i += 1;
                }
            }
            0x04 => {
                i += 1;
                let start = if i < bytes.len() && bytes[i] == b'#' { i + 1 } else { i };
                let mut j = start;
                while j < bytes.len() && j < start + 6 && bytes[j].is_ascii_hexdigit() {
                    j += 1;
                }
                i = j;
            }
            _ => {
                let ch = s[i..].chars().next().unwrap_or('\0');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Wrap styled spans to fit width, splitting spans as needed.
pub fn wrap_spans(spans: &[Span<'static>], max_width: usize) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![Line::from(spans.to_vec())];
    }
    let mut lines = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut line_width = 0;
    let mut run = String::new();
    let mut run_width: usize = 0;
    let mut run_style = Style::default();

    for span in spans {
        let content = span.content.as_ref();
        let style = span.style;
        for ch in content.chars() {
            let cw = ch.width().unwrap_or(1);
            if line_width + run_width + cw > max_width && !run.is_empty() {
                if line_width > 0 {
                    current_line.push(Span::styled(std::mem::take(&mut run), run_style));
                    run_width = 0;
                    lines.push(Line::from(std::mem::take(&mut current_line)));
                    line_width = 0;
                } else {
                    current_line.push(Span::styled(std::mem::take(&mut run), run_style));
                    lines.push(Line::from(std::mem::take(&mut current_line)));
                    run_width = 0;
                }
            }
            if !run.is_empty() && run_style != style {
                current_line.push(Span::styled(std::mem::take(&mut run), run_style));
                line_width += run_width;
                run_width = 0;
            }
            run_style = style;
            run.push(ch);
            run_width += cw;
        }
    }
    if !run.is_empty() {
        current_line.push(Span::styled(run, run_style));
    }
    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }
    if lines.is_empty() && !spans.is_empty() {
        lines.push(Line::from(spans.to_vec()));
    }
    lines
}
