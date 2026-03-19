# rvIRC

> ⚠️ **Active development** — This project is under active development. APIs, behavior, and features may change. Use at your own risk.

**Rust + VIM + IRC** — A terminal IRC client with vim-style modes and commands, built with [ratatui](https://ratatui.rs/) and the [irc](https://crates.io/crates/irc) crate.

![rvIRC](images/rvirc.png)

## Requirements

- **Rust** (latest stable): <https://rustup.rs/>

## Build & run

```bash
cargo build --release
./target/release/rvirc
```

---

## Features

| Feature | Description |
|---------|-------------|
| **Vim-style modes** | NORMAL, INSERT, COMMAND with distinct status bar colors (blue, green, orange) |
| **Multi-server** | Connect to multiple IRC servers simultaneously; channels pane groups them by server |
| **Config** | `~/.config/rvIRC/config.toml` — multiple servers, nickname, optional NickServ identify and auto-join. Per-server auto-connect |
| **Auto-reconnect** | After unexpected disconnect, retries up to 3 times (5s, 15s, 30s). Manual `:connect` or `:quit` cancels |
| **Encrypted DMs** | End-to-end encrypted DM via `:secure`. X25519 + ChaCha20-Poly1305, directional keys, TOFU, SAS verification. Lock icon and checkmark in channel list |
| **File transfer** | Send files via `:sendfile` using [magic-wormhole](https://crates.io/crates/magic-wormhole). Accept/reject popup; in-chat progress |
| **Inline images** | Inline display of image URLs in chat via [ratatui-image](https://crates.io/crates/ratatui-image) (Sixel, Kitty, iTerm2). Animated GIFs supported |
| **Notifications** | Desktop notifications and optional sound for unviewed buffers. `:notifications on\|off`, `:mute` / `:unmute` |
| **Friends list** | Track nicks with `:add-friend` / `:remove-friend`. Green = online, yellow = away, red = offline. Uses MONITOR + away-notify |
| **Superlist** | `:superlist` fetches channel list from all connected servers in one combined, filterable window |
| **IRC formatting** | Bold, italic, strikethrough, colors. `*italic*`, `**bold**`, `~~strikethrough~~`, `\|\|spoiler\|\|`, `:colorname:` — rendered from [IRC format codes](https://modern.ircdocs.horse/formatting) |
| **rvIRC effects** | `@@text@@` = animated rainbow; `$$text$$` = scared flicker — sent as literal text, only rvIRC renders them |
| **Search** | `:search` opens a filterable popup over the current buffer; jump to any result |
| **Highlight words** | `:highlight` — manage a list of words that trigger mention-style notifications and highlighting |

---

## IRCv3 Capabilities

rvIRC requests and implements the full deployed IRCv3 capability surface. Capabilities are negotiated per-server; `:caps` shows what each server acked.

### Stable Capabilities

| Capability | What rvIRC does |
|------------|----------------|
| **sasl** | SASL PLAIN and EXTERNAL authentication (set `sasl_mechanism` in server config) |
| **multi-prefix** | User list shows all mode prefixes per nick (e.g. `@+nick`) |
| **away-notify** | Friends pane updates yellow/green in real time as nicks go away and come back |
| **account-notify** | Tracks nick→account mapping changes (login/logout) |
| **account-tag** | Shows account name alongside messages where available |
| **extended-join** | Account name and real name received on JOIN; used for account display |
| **invite-notify** | Notifies you in the status bar when someone else is invited to a channel you're in |
| **chghost** | Updates the ident@host cache when a nick changes host (no QUIT+JOIN needed) |
| **cap-notify** | Handles CAP NEW/DEL: dynamically updates the displayed cap list |
| **userhost-in-names** | User list displays `nick!user@host`; userhosts stored for whois and display |
| **server-time** | All messages display the timestamp provided by the server (`time` tag) |
| **message-tags** | Full message-tag support: typing indicators, reactions, replies, bot flags, and more |
| **batch** | Batches chathistory delivery; netsplit/netjoin batches collapsed into one summary line |
| **echo-message** | Server echoes sent messages (ensures correct msgids for threading and editing) |
| **labeled-response** | Tags requests to correlate server responses |
| **standard-replies** | FAIL/WARN/NOTE structured replies shown with color-coded `[FAIL]`/`[WARN]`/`[NOTE]` labels |
| **setname** | `:setname <realname>` changes your real name; others' setname events displayed in chat |

### Working Group / Draft Capabilities

| Capability | What rvIRC does |
|------------|----------------|
| **extended-monitor** | Extended MONITOR for friend presence tracking; `:add-friend` / `:remove-friend` manage the list |
| **draft/chathistory** | Fetches last N messages on channel join; `:more` / `:history` fetches older messages (CHATHISTORY BEFORE) |
| **draft/read-marker** | Sends MARKREAD when switching buffers; incoming MARKREAD from other clients syncs read state |
| **draft/multiline** | Pasting multi-line text sends it as a `draft/multiline` BATCH instead of separate messages |
| **draft/pre-away** | Sends AWAY before CAP END on reconnect so you're immediately away if you were before disconnect |
| **draft/no-implicit-names** | Suppresses the server's automatic NAMES reply on JOIN (rvIRC requests it explicitly) |
| **draft/channel-rename** | RENAME command moves the channel in all panes, preserves history under the new name |
| **draft/message-redaction** | `:redact` — sends REDACT to the server; incoming REDACT replaces message text with `[Message redacted]` |
| **draft/message-edit** | `:edit` — sends EDIT with `draft/target-msgid` and `draft/edit-text` tags; incoming edits update text in-place with `(edited)` marker |
| **draft/message-delete** | `:delete` — sends DELETE with `draft/target-msgid` tag; incoming deletes replace text with `[Message deleted]` |
| **draft/account-registration** | `:register <email> <password>` sends REGISTER; result shown via standard-replies (FAIL/NOTE) |
| **draft/extended-isupport** | Requests early ISUPPORT delivery; parses `NETWORK=` and `NETWORKICON=` tokens |
| **draft/metadata** | `:metadata` GET/SET/CLEAR/LIST for key-value user metadata; `display-name`, `pronouns`, and `avatar` fetched automatically on `:whois` |
| **account-extban** | `:bans` / `:banlist` shows scrollable ban-list popup; `$a:account` extbans rendered as `[account: name]` |

### Message Interaction (Client Tags — no cap required)

| Tag / Spec | What rvIRC does |
|------------|----------------|
| **+reply** | `:reply` — `r` in NORMAL mode shows numbers 1–9, 0 on the last 10 messages; press a number to thread a reply |
| **+draft/react** | `:react <emoji>` — sends a TAGMSG reaction; incoming reactions displayed inline below the target message |
| **+draft/channel-context** | DMs from bots that include a channel context appear in the channel buffer, not a DM window |
| **+typing** | Typing indicator: rvIRC sends `active`/`done` events as you type; displays other users' typing status below the input bar |

### Server Behavior (ISUPPORT tokens)

| Token | What rvIRC does |
|-------|----------------|
| **UTF8ONLY** | Shows a notice in the server pane when the server enforces UTF-8 |
| **NETWORK=** | Network name shown in the server pane on connect |
| **NETWORKICON=** | Network icon URL shown in the server pane on connect |
| **WHOX** | Extended WHO on join fetches each user's account name for account-extban display |
| **STS** | Persists STS upgrade policies; forces TLS on future connections to that host |

---

## Commands

Type `:` to enter COMMAND mode, or `/` in the input bar (e.g. `/join #rust`). Commands are case-insensitive. Tab-completion works on command names.

### Connection

| Command | Description |
|---------|-------------|
| `connect <name>` / `server <name>` | Connect to a server by config name |
| `servers` | Show server list from config; pick one to connect |
| `reconnect` | Reconnect to the current server |
| `disconnect [server]` | Disconnect from current server, or the named server |
| `quit` / `exit` / `q [message]` | Disconnect all and quit; optional quit message shown to other users |

### Channels & Messaging

| Command | Description |
|---------|-------------|
| `join #channel [key]` | Join a channel (`#` prepended if omitted); optional key for keyed channels |
| `part` / `leave [#channel]` | Part current channel; or close current DM window |
| `list [server]` | Fetch and show channel list from current server or named server. Type to filter, Enter to join |
| `superlist` | Fetch channel list from all connected servers; combined filterable window |
| `msg <nick> <text>` / `query` / `message` | Send a private message and open a DM window |
| `me <action>` | Send a CTCP ACTION (`/me`) to the current channel or DM |
| `topic` | Show current channel topic; `topic <text>` sets it (if you have permission) |
| `invite <nick> [#channel]` | Invite nick to channel (defaults to current channel) |
| `channel #chan` / `chan #chan` / `c #chan` | Switch to channel or DM by name |
| `clear` | Clear all messages in the current channel/DM buffer |

### Users

| Command | Description |
|---------|-------------|
| `nick <newnick>` | Change your nickname |
| `away [message]` | Set away status with message; no message clears away |
| `whois [nick]` | Show whois popup; omit nick to use the current DM target |
| `kick [#channel] <nick> [reason]` | Kick user from channel (defaults to current channel) |
| `ban [#channel] <mask>` | Set a ban mask (e.g. `*!*@host` or `nick!*@*`) |
| `unban [#channel] <mask>` | Remove a ban mask |
| `bans` / `banlist [#channel]` | Fetch and display the ban list in a scrollable popup |
| `ignore <nick>` | Locally hide all messages from a nick in all buffers |
| `unignore <nick>` | Remove a nick from the local ignore list |
| `mute` | Mute a nick in the current buffer only (hides their messages locally) |
| `unmute` | Unmute a previously muted nick in the current buffer |
| `add-friend <nick>` | Add nick to the friends list (MONITOR for presence) |
| `remove-friend <nick>` | Remove nick from the friends list |

### Account

| Command | Description |
|---------|-------------|
| `pass <password>` / `pass <service> <password>` | Identify with NickServ (default) or another service |
| `register <email> <password>` | Register a new account (requires `draft/account-registration`); result shown via standard-replies |
| `setname <realname>` | Change your IRC real name (requires `setname` cap) |

### Message Actions

| Command | Description |
|---------|-------------|
| `reply` | Enter reply-select mode — numbers 1–9, 0 appear on the last 10 messages; press a digit to set the reply target, then type and send |
| `react <emoji>` | Send a reaction to the selected message (use `r` to select first, or `:reply` to pick one) |
| `redact [msgid] [reason]` | Redact a message — omit msgid to redact your last sent message; or select with `r` first |
| `edit <new text>` | Edit your last sent message; or `edit msgid=<id> <new text>` for a specific message (requires `draft/message-edit`) |
| `delete [msgid]` | Delete your last sent message; or `delete <msgid>` for a specific message (requires `draft/message-delete`) |
| `more` / `history` | Fetch older messages in current channel/DM (CHATHISTORY BEFORE) |
| `search` | Open search popup over current buffer; type to filter, Enter to browse and jump |

### Metadata

| Command | Description |
|---------|-------------|
| `metadata list` | List all your own metadata keys |
| `metadata get <key>` | Get a specific metadata key (own nick) |
| `metadata set <key> <value>` | Set a metadata key/value |
| `metadata clear [key]` | Clear one key or all metadata |
| `metadata <target> list` | List all metadata for a target nick or channel |
| `metadata <target> get <key>` | Get a specific metadata key for a target |
| `metadata <target> set <key> <value>` | Set metadata on a target (if permitted) |
| `metadata <target> clear [key]` | Clear metadata on a target |

Metadata keys `display-name`, `pronouns`, and `avatar` are fetched automatically when you run `:whois` and shown in the popup.

### Encrypted DMs & File Transfer

| Command | Description |
|---------|-------------|
| `secure [nick]` | Initiate an encrypted DM session (defaults to current DM target) |
| `unsecure [nick]` | End an encrypted DM session |
| `verify [nick]` | Display a 6-word SAS verification code for the current secure session |
| `verified [nick]` | Mark the peer as verified after comparing codes out-of-band |
| `sendfile [nick] [path]` | Send a file via magic wormhole; omit path to browse, omit nick to use current DM |

### UI & Other

| Command | Description |
|---------|-------------|
| `channel-panel show\|hide` | Show or hide the channels pane |
| `messages-panel show\|hide` | Show or hide the messages (DMs) pane |
| `user-panel show\|hide` | Show or hide the users pane |
| `friends-panel show\|hide` | Show or hide the friends pane |
| `channels` / `messages` / `users` / `friends` | Focus the corresponding pane |
| `highlight` | Open the highlight words popup; add words that trigger mention-style alerts |
| `notifications on\|off` | Enable or disable desktop notifications |
| `mute` / `unmute` | Mute or unmute notification sound globally |
| `caps` | Show negotiated IRCv3 caps for the current server in the status bar |
| `raw <IRC command>` | Send a raw IRC command (e.g. `:raw MODE #chan +b *!*@badhost`) |
| `version` | Show version in status bar |
| `credits` | Show credits popup |
| `license` | Show license popup (scrollable) |

---

## Keybindings

### NORMAL mode

| Key | Action |
|-----|--------|
| `i` | Enter INSERT mode (type messages) |
| `:` | Enter COMMAND mode |
| `r` | Reply/select mode — numbers appear on last 10 messages; press 1–9 or 0 to pick, then `i` to type and Enter to send |
| `c` | Focus channels pane |
| `m` | Focus messages (DMs) pane |
| `u` | Focus users pane |
| `f` | Focus friends pane |
| `k` / `j` or ↑ / ↓ | Scroll message area (when focused on main) |
| Page Up / Page Down | Scroll message area by full page |
| Esc | Return focus to main message area |
| Ctrl+C | Quit the app |

### INSERT / COMMAND mode

| Key | Action |
|-----|--------|
| Enter | Send message / execute command |
| Esc | Return to NORMAL mode |
| ↑ / ↓ | Browse input history |
| Tab | Tab-complete command names (after `:`) |
| Ctrl+A / Home | Move cursor to start of input |
| Ctrl+E / End | Move cursor to end of input |
| Ctrl+W | Delete word to the left |
| Ctrl+U | Delete to start of line |
| Ctrl+K | Delete to end of line |

### Channels pane (focused)

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Enter | Switch to selected channel / server buffer |
| `c` / Esc | Unfocus pane |

### Messages pane (focused)

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Enter | Switch to selected DM |
| `m` / Esc | Unfocus pane |

### Users pane (focused)

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Type to filter | Filter user list by nick |
| Enter | Open user action menu |
| `u` / Esc | Unfocus pane |

User action menu: **DM** (open DM window), **Kick**, **Ban**, **Unban**, **Op**, **Deop**, **Voice**, **Devoice**, **Halfop**, **Dehalfop** (IRC mode commands on current channel), **Mute** (local hide), **Whois** (popup).

### Friends pane (focused)

Names are colored by status: green = online, yellow = away, red = offline. Config `offline_friends = "hide"` removes offline friends from the list.

| Key | Action |
|-----|--------|
| `k` / `j` or ↑ / ↓ | Move selection |
| Enter | Open DM with selected friend |
| `f` / Esc | Unfocus pane |

### Popups

- **:servers** — `j`/`k` or arrows to move, Enter to connect, Esc to close.
- **:list** / **:superlist** — Type to filter; Enter toggles scroll mode; `j`/`k` + Enter to join; Esc to close.
- **:search** — Type to filter; Enter browses results; `j`/`k` + Enter to jump to message; Esc to close.
- **:whois** — Shows nick, host, server, channels, idle time, and metadata (display-name, pronouns, avatar). Esc / Enter / `q` to close.
- **:bans / :banlist** — `j`/`k` or arrows to scroll; Esc / Enter / `q` to close. `$a:account` extbans shown as `[account: name]`.
- **:highlight** — Type to add a word, Enter to add; `j`/`k` to select, `x` / Delete to remove; Esc to close.
- **:credits** / **:license** — `j`/`k` / Page Up/Down to scroll license; Esc / Enter / `q` to close.
- **Secure session request** — `y` / Enter to accept, `n` / Esc to reject. Red TOFU warning if the peer's identity key changed.
- **File receive offer** — `y` / Enter to accept, `n` / Esc to reject.
- **File browser** — `j`/`k` to navigate; Enter to open directory (or select file for send); Backspace to go up; `s` to save here (receive); Esc / `q` to cancel.

---

## Config

Config path: `~/.config/rvIRC/config.toml`. If missing, the directory and a default config are created with 0600 permissions on Unix.

> **Security**: `config.toml` may contain `identify_password` and server `password`. Ensure it is not world-readable (`chmod 600 ~/.config/rvIRC/config.toml`).

Files stored in the same config directory:

| File | Contents |
|------|----------|
| `config.toml` | Main config: servers, nickname, preferences |
| `identity.toml` | Persistent X25519 identity keypair (auto-generated, 0600 on Unix) |
| `known_keys.toml` | TOFU key store: peer identity keys, verification status, timestamps |
| `friends.toml` | Friends list (nicks to MONITOR) |
| `highlight.toml` | Highlight word list |
| `read_markers.toml` | Per-buffer read position |
| `sts.toml` | Persisted STS upgrade policies (host → port + expiry) |

```toml
username = "myuser"
nickname = "mynick"
# alt_nick = "mynick_"            # optional: fallback nick if primary is in use (433)
real_name = "My Name"
# download_dir = "~/Downloads/"   # optional: default save directory for received files
# render_images = true            # optional: set false to disable inline image display (default: true)
# offline_friends = "show"        # optional: "show" (red) or "hide" — hides offline friends (default: show)
# notifications = true            # optional: desktop notifications for other buffers (default: true)
# sounds = true                   # optional: terminal bell with notifications (default: true)

[[servers]]
name = "Libera"
host = "irc.libera.chat"
port = 6697
tls = true
# identify_password = "your_nickserv_password"   # optional: identify with NickServ after connect
# sasl_mechanism = "plain"                       # optional: "plain" or "external"
# auto_connect = "yes"                           # optional: connect on startup
# auto_join = "#rvirc, #rust"                    # optional: join after identify
# proxy_url = "socks5://127.0.0.1:1080"          # optional: SOCKS5 proxy

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

Connect flow: TCP connect → TLS (if enabled) → CAP negotiation → SASL (if configured) → NickServ identify (if `identify_password` set) → auto-join channels.

---

## Encrypted DMs

rvIRC supports end-to-end encrypted direct messages between rvIRC clients. This is an rvIRC-exclusive feature that works transparently over standard IRC without any server changes.

### Workflow

1. Open a DM with the target user and run `:secure` (or `:secure <nick>` from anywhere)
2. A fresh ephemeral X25519 keypair is generated for this session
3. Both clients exchange ephemeral + identity public keys via hidden protocol messages
4. The recipient sees an accept/reject popup before completing the handshake
5. TOFU checks the peer's identity key against `known_keys.toml` — a warning is shown if the key changed since last session
6. In-chat status messages show handshake progress, key fingerprints, and TOFU result
7. Once established, all messages in that DM are encrypted with ChaCha20-Poly1305 using directional keys
8. A lock icon (🔒) appears next to the nick in the channel list; a checkmark (✔) if the peer is verified
9. Run `:verify` to display a 6-word SAS code — compare out-of-band with your peer, then `:verified` to mark them trusted
10. Run `:unsecure` to end the session

### Security model

- **Persistent identity**: X25519 keypair stored in `identity.toml` (0600). Used for TOFU and SAS — not for DH (each session uses a fresh ephemeral keypair).
- **Directional keys**: The DH shared secret is expanded via HKDF-SHA256 into two independent keys (`rvIRC-dm-init` and `rvIRC-dm-resp`) assigned by lexicographic ordering of ephemeral public keys. Each side uses a different key to send, preventing (key, nonce) reuse.
- **TOFU**: Peer identity keys are stored in `known_keys.toml` with nick, server, fingerprint, verification status, and timestamps. Key changes trigger a red warning and block the ACK path.
- **SAS verification**: `:verify` derives a 6-word Short Authenticated String from the DH secret + both identity keys via HKDF. Both sides should see the same 6 words if there is no MITM. Run `:verified` to mark as trusted.
- **Replay protection**: Received messages must arrive with monotonically increasing nonces. Out-of-order delivery (network splits, relays) may cause legitimate messages to be rejected — run `:secure` again to re-key.

Protocol messages use a `[:rvIRC:]` prefix, are intercepted before display, and never shown to the user. Inline image display works inside encrypted DMs.

---

## File Transfer

rvIRC clients can send files to each other using [magic-wormhole](https://crates.io/crates/magic-wormhole):

1. Sender runs `:sendfile <nick> /path/to/file`, or `:sendfile` in a DM to open a file browser
2. A wormhole code is generated and sent to the recipient via an rvIRC protocol message
3. The recipient sees a popup: `nick wants to send you file.txt (1.2 MB). Accept?`
4. If accepted: file is saved to `download_dir` if configured, otherwise a directory browser appears
5. A progress popup shows transfer bytes during the wormhole relay

Works inside encrypted DM sessions (the wormhole code itself is encrypted in transit).

---

## License

See [LICENSE](LICENSE) for details.
