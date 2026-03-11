# rvIRC

**Rust + VIM + IRC** — A terminal IRC client with vim-style modes and commands, built with [ratatui](https://ratatui.rs/) and the [irc](https://crates.io/crates/irc) crate.

## Requirements

- **Rust** (latest stable): <https://rustup.rs/>

## Features

- **Vim-style modes**: NORMAL, INSERT, COMMAND with distinct status bar colors (blue, green, orange).
- **Commands**: `:connect`, `:servers`, `:join`, `:list`, `:part`, `:msg`, `:quit`, and more (see below).
- **Panes**: Channels (left) and users (right). Toggle with `c` / `u`; j/k or arrows + Enter to switch channel or open user actions (DM, whois, etc.).
- **Config**: `~/.config/rvIRC/config.toml` — multiple servers, nickname, optional NickServ identify and auto-join. Connect order: connect → identify with NickServ (if set) → then auto-join channels.

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
| `join #channel` | Join a channel (`#` added if omitted) |
| `part` / `leave` | Part current channel; `part #chan` parts specific channel |
| `list` | Fetch and show channel list (popup); type to filter, Enter to join |
| `msg <nick> <text>` / `query` | Send a private message |
| `channel #chan` / `chan` / `c #chan` | Switch to channel/DM by name |
| `quit` / `exit` / `q` | Disconnect and quit |
| `channel-panel show` / `hide` | Show or hide the channels pane |
| `user-panel show` / `hide` | Show or hide the users pane |
| `channels` / `users` | Focus channels or users pane |

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

### Popups

- **:servers** — j/k or arrows to move, **Enter** to connect, **Esc** to close.
- **:list** — Type to filter; **Enter** to toggle “scroll mode” then j/k + Enter to join; **Esc** to close.
- **Whois** — **Esc** or **Enter** or **q** to close.

## Config

Config path: `~/.config/rvIRC/config.toml`. If missing, the app creates the directory and a default config.

```toml
username = "myuser"
nickname = "mynick"
real_name = "My Name"

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

## License

See [LICENSE](LICENSE) for details.
