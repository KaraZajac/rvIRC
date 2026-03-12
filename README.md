# rvIRC

**Rust + VIM + IRC** — A terminal IRC client with vim-style modes and commands, built with [ratatui](https://ratatui.rs/) and the [irc](https://crates.io/crates/irc) crate.

![rvIRC](images/rvirc.png)

## Requirements

- **Rust** (latest stable): <https://rustup.rs/>

## Features

- **Vim-style modes**: NORMAL, INSERT, COMMAND with distinct status bar colors (blue, green, orange).
- **Commands**: `:connect`, `:servers`, `:join`, `:list`, `:part`, `:msg`, `:quit`, and more (see below).
- **Panes**: Channels (left) and users (right). Toggle with `c` / `u`; j/k or arrows + Enter to switch channel or open user actions (DM, whois, etc.).
- **Config**: `~/.config/rvIRC/config.toml` — multiple servers, nickname, optional NickServ identify and auto-join. Connect order: connect → identify with NickServ (if set) → then auto-join channels.
- **Auto-reconnect**: After an unexpected disconnect, the client retries up to 3 times (5s, 15s, 30s). Manual `:connect` or `:quit` cancels auto-reconnect.
- **Encrypted DMs**: Two rvIRC clients can establish an end-to-end encrypted DM session using `:secure` (or `:secure <nick>`). Uses X25519 key exchange + ChaCha20-Poly1305 encryption. A green lock icon appears in the channel list next to secure sessions. In-chat status messages show the handshake progress.
- **File Transfer**: Send files between rvIRC clients using `:sendfile` (opens a file browser) or `:sendfile <nick> <path>`. Uses [magic-wormhole](https://crates.io/crates/magic-wormhole) for secure relay-based file transfer. The recipient gets a popup to accept or reject the file. In-chat status messages track transfer progress.
- **Message area**: Long messages wrap to the pane width. The view auto-scrolls to the bottom when new messages arrive (scroll up with k/j or Page Up/Down to read history). Links that end in an image extension (e.g. `.png`, `.jpg`, `.gif`) are fetched and displayed inline in the chat using [ratatui-image](https://crates.io/crates/ratatui-image) when your terminal supports it (e.g. Sixel, Kitty, iTerm2); this works in channels, DMs, and encrypted DMs (animated GIFs display as a static frame).

## Build & run

```bash
cargo build --release
./target/release/rvirc
```

## Commands

Type `:` to enter COMMAND mode, then run any of these (case-insensitive):

| Command | Description |
|--------|-------------|
| `connect <name>` / `server <name>` | Connect to a server by config name |
| `servers` | Show server list from config; pick one to connect |
| `reconnect` | Reconnect to the current server |
| `join #channel [key]` | Join a channel (`#` added if omitted); optional key for keyed channels |
| `part` / `leave` | Part current channel; `part #chan` parts specific channel |
| `list` | Fetch and show channel list (popup); type to filter, Enter to join |
| `msg <nick> <text>` / `query` | Send a private message |
| `me <action text>` | Send an action (/me) to the current channel or DM |
| `nick <newnick>` | Change your nickname |
| `topic` | Show current channel topic; `topic <text>` to set it (if op) |
| `kick [channel] <nick> [reason]` | Kick user from channel |
| `ban [channel] <mask>` | Set ban mask on channel (e.g. `*!*@host` or `nick!*@*`) |
| `channel #chan` / `chan` / `c #chan` | Switch to channel/DM by name |
| `quit` / `exit` / `q` | Disconnect and quit |
| `channel-panel show` / `hide` | Show or hide the channels pane |
| `user-panel show` / `hide` | Show or hide the users pane |
| `channels` / `users` | Focus channels or users pane |
| `version` | Show version (1.0.0) in status bar |
| `credits` | Show credits popup (author and GitHub link) |
| `license` | Show license popup (full LICENSE text) |
| `secure [nick]` | Start an encrypted DM session (defaults to current DM) |
| `unsecure [nick]` | End an encrypted DM session (defaults to current DM) |
| `sendfile [nick] [path]` | Send a file via magic wormhole; omit path to browse, omit nick to use current DM |

## Keybindings

### NORMAL mode

| Key | Action |
|-----|--------|
| `i` | Enter INSERT mode (type messages) |
| `:` | Enter COMMAND mode (run commands) |
| `c` | Toggle channels pane / focus channels |
| `u` | Toggle users pane / focus users |
| `k` / `j` or ↑ / ↓ | Scroll message area (when focus on main) |
| Page Up / Page Down | Scroll message area by page |
| Esc | Unfocus channel/user pane (back to main) |
| Ctrl+C | Quit app |

### INSERT / COMMAND mode

- Type your message or command; **Enter** to send, **Esc** to return to NORMAL.
- **↑ / ↓** — Input history (previous/next line).
- **Tab** — Complete `:command` names.

### Channels pane (when focused)

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Enter | Switch to selected channel |
| `c` / Esc | Unfocus pane |

### Users pane (when focused)

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Enter | Open user action menu (DM, Kick, Ban, Mute, Whois) |
| `u` / Esc | Unfocus pane |

User actions: **Kick** and **Ban** perform the IRC command (current channel). **Mute** hides that nick’s messages locally.

### Popups

- **:servers** — j/k or arrows to move, **Enter** to connect, **Esc** to close.
- **:list** — Type to filter; **Enter** to toggle “scroll mode” then j/k + Enter to join; **Esc** to close.
- **Whois** — **Esc** or **Enter** or **q** to close.
- **:credits** — **Esc** or **Enter** or **q** to close.
- **:license** — **j/k** or arrows / Page Up/Down to scroll; **Esc** or **Enter** or **q** to close.
- **File receive** — **y** / **Enter** to accept, **n** / **Esc** to reject.
- **File browser** (receive: choose save dir; send: choose file) — **j/k** or arrows to navigate, **Enter** to open directory (or select file when sending), **Backspace** to go up, **s** to save here (receive mode), **Esc** or **q** to cancel.

## Config

Config path: `~/.config/rvIRC/config.toml`. If missing, the app creates the directory and a default config.

```toml
username = "myuser"
nickname = "mynick"
# alt_nick = "mynick_"   # optional: used if primary nick is in use (433)
real_name = "My Name"
# download_dir = "~/Downloads/"   # optional: default save directory for received files

[[servers]]
name = "Libera"
host = "irc.libera.chat"
port = 6697
tls = true
# identify_password = "your_nickserv_password"   # optional: identify with NickServ after connect
# auto_connect = "yes"                          # optional: connect to this server on startup
# auto_join = "#rvirc, #rust"                    # optional: join these channels after identify

[[servers]]
name = "Hackint"
host = "irc.hackint.org"
port = 6697
tls = true

[[servers]]
name = "Local"
host = "127.0.0.1"
port = 6667
tls = false
```

Connect flow: connect to server → identify with NickServ (if `identify_password` is set) → then auto-join channels from `auto_join`.

The message area shows **channel topic** and **modes** (e.g. `+nt`) when available. Messages that **mention your nick** are highlighted. When someone **invites** you to a channel, the status line shows the invite and you can `:join #channel` to accept. The client replies to **CTCP** VERSION, PING, and TIME.

## Encrypted DMs

rvIRC supports end-to-end encrypted direct messages between rvIRC clients. This is an rvIRC-exclusive feature that works transparently over standard IRC.

1. Open a DM with the target user and run `:secure` (or `:secure <nick>` from anywhere)
2. Both clients exchange X25519 public keys via hidden protocol messages
3. In-chat status messages show the handshake progress (key exchange, success/failure)
4. Once established, all messages in that DM are encrypted with ChaCha20-Poly1305
5. A lock icon (🔒) appears next to the nick in the channels list
6. Use `:unsecure` (or `:unsecure <nick>`) to end the encrypted session

The key exchange and encrypted messages use `[:rvIRC:]`-prefixed protocol messages that are intercepted and never displayed to the user. Inline image display works in encrypted DMs as well.

## File Transfer

rvIRC clients can send files to each other using magic wormhole:

1. Sender runs `:sendfile <nick> /path/to/file`, or `:sendfile` in a DM to open a file browser
2. A wormhole code is generated and sent to the recipient via an rvIRC protocol message
3. In-chat status messages track the transfer progress (connection, sending, completion)
4. The recipient sees a popup: "nick wants to send you file.txt (1.2 MB). Accept?"
5. If accepted and `download_dir` is configured, the file is saved there directly
6. If `download_dir` is not set, a file browser popup lets the user choose a save directory
7. The file transfers securely via the magic wormhole relay

All three commands (`:secure`, `:unsecure`, `:sendfile`) default to the nick of the current DM window when no nick argument is given.

## License

See [LICENSE](LICENSE) for details.
