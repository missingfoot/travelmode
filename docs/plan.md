# Travel Mode — Plan

Rust workspace: root daemon + CLI (Phase 1), GTK4/libadwaita GUI (Phase 2), then the rest of your roadmap.

## Review of your phase plan

Your roadmap is solid. The changes I'd make:

1. **CLI moves from Phase 9 to Phase 1.** You said you want a CLI for testing — the daemon is useless without a client to poke it, so the CLI is built alongside the daemon from day one. Phase 9 keeps the *scheduler*; the CLI itself already exists by then.
2. **Per-application blocking is the one genuinely hard problem in this project** (TripMode's whole trick on macOS uses a kernel API Linux doesn't have). nftables cannot match packets by process name. The realistic options:
   - **NFQUEUE + /proc attribution (OpenSnitch-style)** — new connections are punted to the daemon, which maps socket inode → PID via `/proc/<pid>/fd` and returns allow/deny. Works on *any* running app with no launcher tricks. Cost: userspace round-trip per new connection. This is the proven, pragmatic choice and what I recommend for Phase 1.
   - **cgroup v2 + native nftables matching** (`socket cgroupv2 level N`) — zero per-packet cost, but requires moving apps into managed cgroups, which fights with systemd's own cgroup tree.
   - **eBPF (aya)** — best long-term, highest complexity.
   Plan: build the firewall engine behind a trait, ship NFQUEUE first, keep nftables for static/table plumbing, leave room for eBPF later.
3. **Byte attribution**: per-flow up/down counters come from conntrack (netlink, with `CONNTRACK_ACCOUNTING`-style counters), not from /proc. Interface totals come from rtnetlink. This gives the "↑ 420 KB ↓ 12 MB per connection" data without eBPF.
4. **IPC: Unix socket + length-prefixed JSON first**, not D-Bus. Simpler, works for the CLI immediately, no session-bus dependency for a root daemon. D-Bus (zbus) can be added later as a second transport for desktop integration if wanted.
5. **Domain inspection (Phase 7)** implicitly needs DNS snooping — worth knowing early that it means watching UDP/TCP 53 (or DoH caveats). Not a Phase 1 concern, just flagging the hidden dependency.
6. **Phase order otherwise stays as you wrote it.** Profiles (4) before smart networks (5) before limits (6) is the right dependency order since limits and automation both hang off profiles.

## Technology choices (answering your two questions)

**"Is there an existing firewall we can just use?"** — No, not for per-app control:
- **nftables** is the kernel firewall itself, not a per-app solution — it matches packets (IPs, ports, interfaces), and has no concept of "process". Every per-app tool on Linux is built *on top of* it. We use it as our backend; it's present on every current distro.
- **firewalld** (Fedora/RHEL) and **ufw** (Ubuntu) are zone/service managers on top of nftables for *host* filtering. Their egress policies work on zones and addresses, not processes — they cannot express "block Steam". Nothing to reuse there beyond coexistence (we manage our own nftables table and don't fight them).
- **OpenSnitch** is the one existing per-app application firewall on Linux — but it's a whole *application* (Go daemon + Python UI), not a library we can embed. Its value to us is as proof that the NFQUEUE + /proc attribution design works at scale, and as a reference for its known pitfalls. Douane/lpfw are abandoned.
- eBPF frameworks (Cilium etc.) are server/networking-platforms, not embeddable per-app firewalls.

So: no shortcut exists — the plan builds the thin per-app layer (NFQUEUE verdicts + attribution) ourselves on top of nftables, which is exactly what the existing tools had to do too. It's a few hundred lines, not a firewall-from-scratch.

**"Rust + GTK4/libadwaita for cross-distro compatibility?"** — Yes, with one caveat:
- **Daemon + CLI (Rust)**: compiles to a self-contained native binary with essentially no runtime deps. Runs identically on any distro with a modern kernel and nftables. This is the best-possible compatibility story — better than Python/Go daemons for packaging (no interpreter/runtime).
- **GUI**: GTK4 + libadwaita is packaged on all major current distros (Fedora, Arch, openSUSE, Ubuntu ≥ 22.04, Debian ≥ 12), and gtk4-rs bindings are mature. The caveat: version fragmentation on older LTS releases. The standard answer is **target a modest minimum (libadwaita 1.4, i.e. Ubuntu 24.04 / Debian 13) and ship the GUI as a Flatpak** (GNOME runtime) — Flatpak removes distro-version concerns entirely and is where the GNOME/libadwaita ecosystem lives. Native deb/rpm/AUR packages come later as a bonus.
- Qt6 would be the only real alternative (more native-feeling on KDE), but libadwaita gives you adaptive layout, dark/light mode, and modern widgets for free, and the app is GNOME-style by design (menu-bar-less, toggle-driven like TripMode). Sticking with your choice.

## Architecture

Cargo workspace `travelmode`:

- `crates/core` — shared types (rules, profiles, connection info, IPC protocol messages, serde models). No logic.
- `crates/daemon` (`travelmoded`) — the root service. Owns all networking logic.
- `crates/cli` (`travelmode`) — thin client over the unix socket. All daemon functionality testable from here.
- `crates/gui` (`travelmode-gui`) — GTK4/libadwaita app, added in Phase 2. Stateless client of the daemon, same IPC as CLI.
- `crates/storage` — SQLite layer (Phase 3), separated so daemon code stays clean.

Key crates: `tokio`, `serde`/`serde_json`, `clap` (CLI), `nftnl` or `nftables` (or `nft -j` CLI wrapper initially), `netlink-packet-route`/`rtnetlink`, `netlink-conntrack` (or `procfs` + conntrack events), `procfs`, `rusqlite` (Phase 3), `tracing` + `tracing-journald`, `gtk4`/`libadwaita` + `relm4` (Phase 2).

## Phase 1 — Networking Core (the plan to execute now)

1. **Workspace scaffold** — `Cargo.toml` workspace, the 4 crates above (gui as empty placeholder or deferred), `clippy`/`rustfmt` config, `.gitignore`, README with build/run instructions.
2. **Config + logging** — TOML config at `/etc/travelmode/config.toml` (dev override via `--config`), `tracing` with journald/file output.
3. **IPC layer** — Unix socket at `/run/travelmode/daemon.sock`. Length-prefixed JSON messages defined in `core`: `Ping`, `GetStatus`, `GetConnections`, `GetProcesses`, `ListRules`, `AddRule`, `RemoveRule`, `SetPaused`, plus an event stream (`Subscribe`) the GUI will use in Phase 2. CLI gets a `travelmode status` etc. subcommand per message.
4. **Network detection module** — rtnetlink for interfaces/addresses/gateway/routes; parse DNS from `/etc/resolv.conf`; NetworkManager over D-Bus (`zbus`) for SSID, connection type, metered flag (with graceful fallback if NM absent). Expose via `travelmode network`.
5. **Process detection module** — `procfs` scanner: PID, name, exe path, user, PPID. Refresh loop with new-process detection, exposed via `travelmode ps`.
6. **Connection tracking module** — conntrack netlink event listener (NEW/UPDATE/DESTROY with byte counters) for live flows and per-flow up/down bytes; attribute flow → PID by matching socket inode against `/proc/<pid>/fd` (cached, refreshed on new processes). Aggregation view per app. Expose via `travelmode connections` and `travelmode top`.
7. **Firewall engine** — trait `FirewallBackend` with an nftables implementation:
   - Manage our own `inet travelmode` table (never touch other tables).
   - Output-chain NFQUEUE rule for new egress connections while filtering is active.
   - NFQUEUE worker: for each packet, look up process attribution, check rule set, verdict ACCEPT/DROP. Fail-open config option.
   - Rule CRUD: allow/block, temporary (TTL) and persistent rules, keyed by exe path. Persisted to `/etc/travelmode/rules.json` (moves to SQLite in Phase 3).
   - Global pause/resume switch.
   - CLI: `travelmode block <app>`, `allow`, `rules`, `pause`, `resume`.
8. **Daemon lifecycle** — tokio main loop wiring modules together, graceful shutdown, systemd unit file (`travelmoded.service`, hardening directives, `AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW` option vs root), install notes in README.

**Phase 1 verification**: with the daemon running, `travelmode network`/`ps`/`connections` show live data; blocking curl or a browser by exe path demonstrably stops its new connections while allowed apps keep working; rules survive daemon restart; pause/resume works. Everything tested via CLI, no UI.

## Later phases (kept from your roadmap, adjusted)

- **Phase 2 — GTK4/libadwaita GUI**: `crates/gui`, likely `relm4`. Pages: Dashboard, Applications, Connections, Settings. Subscribes to the daemon event stream (no polling logic of its own). Tray via libappindicator/`ksni` (StatusNotifierItem; note GNOME needs the AppIndicator extension). Dark/light is free from libadwaita — your Phase 10 item mostly disappears.
- **Phase 3 — Data accounting**: `crates/storage` with `rusqlite`; tables apps/sessions/connections/usage/networks; rules.json migrates in.
- **Phase 4 — Profiles**: rule sets + limits per profile, manual switch via CLI/GUI.
- **Phase 5 — Smart networks**: SSID/gateway fingerprint → profile mapping, auto-switch on network change events from Phase 1's detection module.
- **Phase 6 — Data limits**: daily/weekly/monthly, per-network; 75/90/100% thresholds; notify (libnotify) → block actions.
- **Phase 7 — Domain inspection**: DNS snooping + IP↔domain cache, per-app domain explorer.
- **Phase 8 — Reports**: period rollups, CSV/JSON export from the storage layer.
- **Phase 9 — Automation**: scheduler (cron-like timing in the daemon); CLI already exists; optional D-Bus API here.
- **Phase 10 — Polish**: floating monitor, graphs, search/filters, i18n.

## Notes

- Your machine (CachyOS, kernel 7.1.5, nftables, gtk4 4.22, libadwaita 1.9) has everything needed; e.g. conntrack kernel support to verify at implementation time (`/proc/net/nf_conntrack` or conntrack module load).
- OpenSnitch is the closest existing Linux analogue and a useful reference for NFQUEUE attribution pitfalls (short-lived connections, PIDs dying mid-lookup, UDP).
- Everything stays local; no telemetry, per your goals.
