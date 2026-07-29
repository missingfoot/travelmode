# travelmode

Per-application network control for Linux — a TripMode alternative.

travelmode shows you which applications are using the network, how much
data each one moves, and lets you block individual applications with a
single command. Useful on metered connections, tethering, or whenever
you want to know who is talking to the internet.

**Status: Phase 2** — daemon, CLI and GTK4 desktop GUI working end to
end. Profiles and per-network rules are on the roadmap.

## Architecture

```
┌─────────────┐   length-prefixed JSON over        ┌──────────────────────────────┐
│  travelmode │─── a Unix socket ────────────────┐ │  travelmoded (root)          │
│  (CLI)      │    /run/travelmode/daemon.sock   │ │                              │
└─────────────┘                                  │ │  conntrack poller ── flow    │
┌─────────────┐   same IPC (fetch + Subscribe    │ │    bytes, opened/closed      │
│travelmode-  │─── event stream, auto-reconnect)─┘ │  process scanner ── pid,     │
│gui (GTK4/   │                                    │    exe, user                 │
│libadwaita + │                                    │  attribution ── /proc net +  │
│SNI tray)    │                                    │    fd inode mapping          │
└─────────────┘                                    │  rule store ── JSON          │
                                                   │    persistence + TTL         │
                                                   │  firewall ── nftables        │
                                                   │    `inet travelmode` table   │
                                                   │    + NFQUEUE verdicts        │
                                                   └──────────────┬───────────────┘
                                                                  │
                                                        table inet travelmode
                                                        chain output:
                                                          ct state new queue num 0 bypass
```

The daemon only ever touches its own nftables table (`inet travelmode`)
and deletes it on shutdown. New outbound connections are queued to the
daemon, which attributes the socket to a process and drops packets from
blocked executables. Everything fails open: if filtering cannot be set
up (not root, missing kernel support), traffic flows and the daemon
reports `filtering_active = false` instead of breaking your network.

## Build

Requires Rust 1.97+ (edition 2021) and the libnetfilter_queue
development files:

```sh
# Arch / CachyOS
sudo pacman -S nftables libnetfilter_queue

cargo build --workspace --release
```

## Runtime dependencies

- `nftables` (`nft` CLI)
- `libnetfilter_queue` (linked at build time)
- conntrack support in the kernel (`nf_conntrack`)
- NetworkManager (optional — used for Wi-Fi SSID / metered detection;
  everything degrades gracefully without it)

## Usage

Install (or copy) the binaries, then enable the service:

```sh
sudo install -Dm755 target/release/travelmoded /usr/bin/travelmoded
sudo install -Dm755 target/release/travelmode /usr/bin/travelmode
sudo install -Dm644 packaging/travelmoded.service /usr/lib/systemd/system/travelmoded.service
sudo systemctl enable --now travelmoded
```

Then:

```sh
travelmode status           # daemon status
travelmode network          # interfaces, gateway, DNS, SSID, metered
travelmode ps               # processes holding sockets
travelmode connections      # live flows with byte counters
travelmode top              # per-application usage
travelmode rules            # list rules
travelmode block firefox    # block by name or path
travelmode block /usr/bin/steam --temp 3600   # block for one hour
travelmode allow firefox    # explicit allow rule
travelmode remove 3         # remove rule by id
travelmode pause            # pause all filtering
travelmode resume
travelmode watch            # stream live events (JSON lines)
travelmode --json top       # machine-readable output
```

## Desktop app

`travelmode-gui` is a GTK4/libadwaita desktop client (relm4) with a
StatusNotifierItem tray icon. Build requirements: `gtk4` and
`libadwaita-1` development packages (targets the libadwaita 1.4 API),
plus `glib-compile-resources` (from glib2, used to bundle the UI icon
set).

```sh
cargo build --workspace --release
install -Dm755 target/release/travelmode-gui ~/.local/bin/travelmode-gui
```

travelmoded must be running (the GUI is a pure client — with the daemon
down it shows a reconnect banner and keeps retrying):

```sh
travelmode-gui                              # uses /run/travelmode/daemon.sock
travelmode-gui --socket /tmp/travelmode/daemon.sock   # dev socket
```

Pages: Dashboard (network summary, live speeds, session totals),
Applications (per-app traffic with block switches), Connections (live
flow list), Settings (daemon status, global pause, about). The tray
menu offers Open, Pause/Resume Filtering and Quit (the GUI only — the
daemon keeps running). Closing the window hides it to the tray.

### Icons and launcher entry

The app glyph (a paper plane) ships in-repo under `data/icons/` in two
variants — `travelmode-dark.svg` (white glyph, for dark themes) and
`travelmode-light.svg` (#1E1E1E glyph, for light themes) — plus PNGs
rendered at 24/32/48/64/128/256 with `rsvg-convert -w N -h N`. The
tray icon is embedded in the binary and follows the system color
scheme; the window/taskbar icon resolves from the icon theme.

The in-app UI icon set lives in `data/icons/ui/` (18 white-glyph SVGs).
`data/icons/render-ui-icons.sh` renders each one in both variants
(white for dark themes, #1E1E1E for light themes) at 24px and 48px into
`data/icons/ui/rendered/` — the 48px PNGs are embedded in the binary
(row icons as textures, tab icons via a compiled gresource) and follow
the system color scheme live. Re-run the script after editing the SVGs.

```sh
# Window/taskbar icon (per-user, hicolor theme):
./dev/install-icons.sh

# Launcher entry:
cp packaging/com.github.missingfoot.travelmode.desktop ~/.local/share/applications/
# or system-wide: sudo desktop-file-install packaging/com.github.missingfoot.travelmode.desktop
```

## Development

You do not need to touch your real firewall to hack on travelmode.
Point the daemon at /tmp with a custom config:

```sh
mkdir -p /tmp/travelmode
cat > /tmp/travelmode/config.toml <<'EOF'
socket_path = "/tmp/travelmode/daemon.sock"
rules_file  = "/tmp/travelmode/rules.json"
log_level   = "debug"
EOF

# As root (conntrack, nft and NFQUEUE need it):
sudo target/debug/travelmoded --config /tmp/travelmode/config.toml

# In another terminal, as your normal user:
target/debug/travelmode --socket /tmp/travelmode/daemon.sock status
```

Without root the daemon still starts and serves IPC — filtering is
simply reported as inactive, which is handy for UI and parser work.

## Tests

```sh
cargo test --workspace
cargo clippy --workspace
```

## Roadmap

- **Phase 1** — daemon + CLI: flow tracking, per-app usage,
  block/allow rules (persistent and temporary), pause/resume, live
  events over IPC.
- **Phase 2 (this)** — GTK4/libadwaita GUI on top of the event stream:
  dashboard with live speeds, per-app block switches, connections list,
  tray icon with pause/resume.
- **Phase 3** — bandwidth rates and per-app history; desktop
  notifications when a new app first uses the network.
- **Phase 4** — profiles per network (different rule sets on home
  Wi-Fi vs. tethering, using the SSID/metered detection already in
  place).

## License

GPL-3.0-or-later.
