//! Desktop notifications and sounds when messages arrive for a buffer you're not viewing.

use notify_rust::Notification;
use std::io::{self, Write};

/// Show a desktop notification for an incoming message.
/// Fails silently if the OS doesn't support notifications or the user has disabled them.
pub fn show_desktop(title: &str, body: &str) {
    let _ = Notification::new()
        .summary(title)
        .body(body)
        .appname("rvIRC")
        .timeout(3000)
        .show();
}

/// Play the terminal bell (ASCII BEL). Works in most terminals.
pub fn play_bell() {
    let _ = io::stdout().write_all(b"\x07");
    let _ = io::stdout().flush();
}
