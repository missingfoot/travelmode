# travelmode

Per-application network control for Linux — a TripMode alternative.

travelmode shows you which applications are using the network, how much
data each one moves, and lets you block individual applications with a
single command. Useful on metered connections, tethering, or whenever
you want to know who is talking to the internet.

**Status: Phase 1** — daemon + CLI working end to end. GUI, profiles,
and per-network rules are on the roadmap.

## Architecture

```
┌─────────────┐   length-prefixed JSON over        ┌──────────────────────────────┐
│  travelmode │─── a Unix socket ─────────────────▶│  travelmoded (root)          │
│  (CLI)      │    /run/travelmode/daemon.sock     │                              │
└─────────────┘                                    │  conntrack poller ── flow    │
                                                   │    bytes, opened/closed      │
                                                   │  process scanner ── pid,     │
                                                   │    exe, user                 │
                                                   │  attribution ── /proc net +  │
                                                   │    fd inode mapping          │
                                                   │  rule store ── JSON          │
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

- **Phase 1 (this)** — daemon + CLI: flow tracking, per-app usage,
  block/allow rules (persistent and temporary), pause/resume, live
  events over IPC.
- **Phase 2** — GUI on top of the event stream; desktop notifications
  when a new app first uses the network.
- **Phase 3** — bandwidth rates and per-app history.
- **Phase 4** — profiles per network (different rule sets on home
  Wi-Fi vs. tethering, using the SSID/metered detection already in
  place).

## License

GPL-3.0-or-later.
