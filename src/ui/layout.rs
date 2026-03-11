//! Compute layout: center area with optional left/right panes.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Returns (center_rect, left_rect_option, right_rect_option).
pub fn center_with_side_panes(
    area: Rect,
    left_width: Option<u16>,
    right_width: Option<u16>,
) -> (Rect, Option<Rect>, Option<Rect>) {
    let l = left_width.unwrap_or(0);
    let r = right_width.unwrap_or(0);

    let constraints = match (l > 0, r > 0) {
        (true, true) => vec![
            Constraint::Length(l),
            Constraint::Min(10),
            Constraint::Length(r),
        ],
        (true, false) => vec![Constraint::Length(l), Constraint::Min(10)],
        (false, true) => vec![Constraint::Min(10), Constraint::Length(r)],
        (false, false) => vec![Constraint::Min(10)],
    };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area);

    match (l > 0, r > 0) {
        (true, true) => (chunks[1], Some(chunks[0]), Some(chunks[2])),
        (true, false) => (chunks[1], Some(chunks[0]), None),
        (false, true) => (chunks[0], None, Some(chunks[1])),
        (false, false) => (chunks[0], None, None),
    }
}
