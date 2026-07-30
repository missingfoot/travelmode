# Travel Mode — Project Status

*Last updated: 2026-07-30. Read this first when picking the project back up.*

A Linux-first, open-source alternative to TripMode: per-application network
access control, live traffic monitoring, and (later) data limits and
profiles. Rust workspace; root daemon + CLI + GTK4/libadwaita GUI.

Repo: https://github.com/missingfoot/travelmode

---

## What's done

### Phase 1 — Networking core (complete, verified)

- **`crates/core`** — shared types + IPC protocol. Length-prefixed JSON
  (4-byte BE length + JSON) over a Unix socket (`/run/travelmode/daemon.sock`,
  mode 0666 so unprivileged clients can talk to the root daemon).
  Request/Response one-shot per connection + `Subscribe` event stream.
- **`crates/daemon` (`travelmoded`)** — root service:
  - TOML config (defaults `/etc/travelmode/config.toml`, `--config` override,
    `deny_unknown_fields`), tracing logging, graceful SIGTERM/SIGINT shutdown
    (removes nft table, unlinks socket, saves rules, exits promptly).
  - Network detection: rtnetlink interfaces/routes/gateway, resolv.conf DNS,
    NetworkManager SSID/metered via zbus (all degrade gracefully).
  - Process discovery via procfs (only socket-holding processes tracked).
  - Connection tracking: polls `/proc/net/nf_conntrack` 1/s (enables
    `nf_conntrack_acct` at startup for byte counters), attributes flows to
    processes via socket inode → `/proc/<pid>/fd` scan, with one-shot
    `/proc/<pid>` fallback for short-lived processes.
  - Firewall: own `inet travelmode` nftables table only; output chain queues
    `ct state new` to NFQUEUE 0 (`bypass`); verdict worker attributes packet
    by source port and DROPs if the exe has a Block rule, else ACCEPT.
    Fail-open everywhere. Rules: persistent + TTL, JSON at
    `/etc/travelmode/rules.json`. Global pause/resume (chain flush/re-add).
  - **Kill-on-block**: adding a Block rule (and startup enforcement of
    persisted rules) kills the app's existing flows — TCP via `ss -K`,
    conntrack entries via `conntrack -D` (optional dep; without it UDP
    lingers until ct timeout).
  - Blocked-attempt logging is rate-limited: first per (exe, dst) at info,
    5s-window summaries after; per-packet detail at debug.
- **`crates/cli` (`travelmode`)** — `status network ps connections top rules
  block allow remove pause resume watch`, `--json`, `--socket`. Block/allow
  resolve bare names via PATH or the daemon's process list; `--temp` for TTL.
- **`packaging/travelmoded.service`** — systemd unit.
- **`dev/verify.sh`** — root verification: 15/15 checks (block, pause/resume,
  persistence across restart, teardown) + a mid-transfer kill check (skips if
  the test host is unreachable). `dev/config.toml` runs everything on /tmp.
- 48 unit tests, clippy clean.

### Phase 2 — GTK4/libadwaita GUI (complete, in daily use)

- **`crates/gui` (`travelmode-gui`)** — relm4 + libadwaita, 1.4 API floor
  (`gnome_45` feature gates newer APIs at compile time). Stateless client;
  background thread with own tokio runtime does subscribe-first + one-shot
  fetches, auto-reconnect with banner.
- Pages: **Dashboard** (network info, live speeds, session totals),
  **Applications** (Allowed/Blocked sections, hidden when empty, live
  re-sort by download bytes, light-switch toggles ON=allowed),
  **Connections** (TCP/UDP/Other sections, live sort), **Settings**
  (daemon status, pause switch, about).
- System tray via ksni (StatusNotifierItem): Open / Pause Filtering / Quit.
- Custom icon set (`data/icons/ui/*.svg` → `render-ui-icons.sh` → PNGs,
  dark+light variants, embedded; tabs via gresource/IconTheme). Icons follow
  system theme live. Plane app icon; `.desktop` file + `dev/install-icons.sh`.
- 16 GUI tests (pure reducer + formatting + icons).

---

## How to run

```bash
cargo build --workspace
sudo ./target/debug/travelmoded        # terminal 1 (root required)
./target/debug/travelmode-gui          # terminal 2
./target/debug/travelmode status       # CLI works too
sudo ./dev/verify.sh                   # full live verification
```

Permanent install: see README ("Permanent setup" — release build,
systemd unit, desktop file, icons).

Runtime deps: nftables (`nft`), libnetfilter_queue, iproute2 (`ss`),
NetworkManager (optional), conntrack-tools (optional, UDP flow-kill).
GUI build needs gtk4 + libadwaita + `glib-compile-resources`.

---

## Known caveats / design decisions to remember

- **Attribution is best-effort.** Flows that can't be attributed show as
  `unknown` and are fail-open (never blocked). Watch the `unknown` row for
  escapes.
- **VPN tunnels**: traffic inside a VPN is seen as the VPN client's
  (e.g. `tailscaled`), not the real app's. Per-app blocking across VPNs is
  unsolved (future idea: VPN awareness).
- Blocking is per **exe path**, exact match. Apps that spawn helpers with
  different binaries need rules per exe.
- Pause = queue rule removed = fully open (no rules evaluated).
- Socket is 0666 — any local user can control the daemon. polkit/group
  auth is a later hardening step.
- GUI: don't re-add status labels/badges to app rows (removed twice —
  sections + switch + dimming carry state). See comment in
  `pages/applications.rs`.

## Icon editing

Edit `data/icons/ui/*.svg` (white `#F7F8F8` only), then
`./data/icons/render-ui-icons.sh && cargo build -p travelmode-gui`.
Keep strokes chunky — thin details alias away at 24px (that's why the
settings gear was replaced). New icon *names* also need entries in
`crates/gui/src/icons.rs` and `crates/gui/ui-icons.gresource.xml`.

---

## What's next (roadmap)

- **Phase 3 — Data accounting (next up)**: `crates/storage` with rusqlite;
  tables apps/sessions/connections/usage/networks; migrate rules.json into
  the DB; per-app totals by session/day/week/month. Everything downstream
  (limits, reports, smart networks) builds on this.
- **Phase 4 — Profiles**: named rule sets + limits, manual switching
  (CLI/GUI). The `profile` field already exists on rules ("default").
- **Phase 5 — Smart networks**: SSID/gateway fingerprint → profile,
  auto-switch on network-change events (netinfo module already refreshes
  every 30s; needs a change watcher + `NetworkChanged` events, which core
  already defines).
- **Phase 6 — Data limits**: daily/weekly/monthly per profile/network;
  75/90/100% warnings; notify → block actions.
- **Phase 7 — Domain inspection**: DNS snooping (UDP/TCP 53; note DoH
  caveat) + IP↔domain cache, per-app domain explorer.
- **Phase 8 — Reports**: period rollups, CSV/JSON export.
- **Phase 9 — Automation**: scheduler in daemon; CLI exists; optional
  D-Bus API (zbus already a dep).
- **Phase 10 — Polish**: floating bandwidth monitor, graphs, search/filter,
  i18n. Dark/light already free from libadwaita.

Long-term ideas: eBPF verdict backend (aya) to replace NFQUEUE, bandwidth
throttling, container/VM awareness, Prometheus metrics.
