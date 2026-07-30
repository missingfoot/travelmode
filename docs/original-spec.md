# Linux Network Control Project
*A Linux-first, open-source alternative to TripMode*

> This is the original feature-set and phase plan as written by the project
> author at kickoff (2026-07-29), preserved verbatim for context. The
> reviewed/approved implementation plan derived from it is in `plan.md`.

## Overview

The goal of this project is to build a lightweight desktop application and background daemon that gives users complete control over application network access and data usage.

Unlike a traditional firewall, the application is designed around **application-level traffic control**, **data usage monitoring**, and **automatic profile switching** based on the connected network.

The architecture is intentionally modular so future features (reports, automation, scheduling, domain inspection, bandwidth limiting, etc.) can be added without redesigning the core.

---

# Project Goals

- Linux-first
- Open Source
- Privacy-focused
- No cloud services
- Local-only data storage
- Low resource usage
- Desktop integration
- Extensible architecture

---

# Planned Features

## Core Features

- Per-application internet access control
- Live traffic monitoring
- Real-time upload/download speeds
- Data usage tracking
- Network profile switching
- Automatic hotspot detection
- Usage history
- Data limits
- Reports
- Scheduler
- Domain inspection
- CLI/API support

---

# Development Roadmap

---

# Phase 1 — Networking Core

## Goal

Build the networking engine that every future feature depends on.

This phase should not focus on UI.

Everything should operate from a background daemon.

---

## Components

### Background Service

Requirements

- Runs as a system daemon
- Starts automatically on boot
- Independent of desktop UI
- Communicates via D-Bus or Unix socket
- Configuration stored locally
- Logging support

---

### Network Detection

Detect

- Active network interfaces
- Ethernet
- Wi-Fi
- VPN
- Mobile tether
- Interface status
- SSID
- Gateway
- DNS servers
- Metered connection (if available)

---

### Process Detection

Maintain a live list of

- PID
- Process name
- Executable path
- User
- Parent process

The daemon should automatically detect newly launched applications.

---

### Connection Tracking

Track every network connection.

Store

- Process ID
- Application
- Local IP
- Local Port
- Remote IP
- Remote Port
- Protocol (TCP/UDP)
- Upload bytes
- Download bytes
- Connection start
- Last activity timestamp

Example

```text
Firefox

TCP
192.168.1.20:51432

↓

github.com:443

↑ 420 KB
↓ 12 MB
```

---

### Firewall Engine

Responsible for allowing or blocking applications.

Operations

- Allow application
- Block application
- Temporary rule
- Persistent rule
- Remove rule

Backend

Primary

- nftables

Fallback

- iptables

Future support

- firewalld integration

---

### Rule Storage

Each rule stores

- Application
- Executable path
- Status
- Profile
- Last modified

Example

```text
Firefox

Allowed

Profile: Default

Updated:
2026-08-01
```

---

## Deliverables

- Background daemon
- Connection tracker
- Process discovery
- Firewall engine
- Rule management
- Configuration system

---

# Phase 2 — Desktop UI

## Goal

Visualise everything the daemon already knows.

The UI should contain **no networking logic**.

---

## Main Window

Pages

- Dashboard
- Applications
- Connections
- Settings

---

## Dashboard

Display

- Current network
- Upload speed
- Download speed
- Total bandwidth
- Active applications
- Blocked applications

---

## Applications Page

Columns

- Icon
- Name
- Upload
- Download
- Status
- Allow/Block toggle

Example

```text
✓ Firefox

↑ 120 MB

↓ 3.2 GB

Allowed
```

```text
✖ Steam

Blocked
```

---

## Live Updates

Refresh

- Every second

No application restart required.

---

## System Tray

Menu

```text
Open

Pause Filtering

Resume Filtering

Quit
```

---

## Deliverables

- GTK/Qt desktop application
- Live process list
- Allow/block controls
- System tray integration

---

# Phase 3 — Data Accounting

## Goal

Persist usage history.

---

## Database

SQLite

Tables

- Applications
- Sessions
- Connections
- Usage
- Networks

---

## Track

Per application

- Upload
- Download

Per

- Session
- Day
- Week
- Month

---

## Session Tracking

Each session stores

```text
Start

End

Apps

Bytes

Profile
```

---

## Statistics

Generate

- Today
- Yesterday
- Week
- Month

---

## Deliverables

- SQLite database
- Historical usage
- Usage calculations

---

# Phase 4 — Profiles

## Goal

Allow multiple firewall configurations.

Profiles

- Home
- Work
- Gaming
- Mobile
- Public Wi-Fi

Each profile stores

- Allowed applications
- Blocked applications
- Limits
- Automation rules

Switching

- Manual
- Automatic
- Network based

---

## Deliverables

- Profile management
- Automatic switching

---

# Phase 5 — Smart Networks

Automatically detect

- Wi-Fi
- Ethernet
- VPN
- Mobile hotspots

Example

```text
Connected to Home Wi-Fi

↓

Activate Home Profile
```

```text
Connected to iPhone Hotspot

↓

Activate Mobile Profile
```

---

## Deliverables

- Network recognition
- Automatic profile activation

---

# Phase 6 — Data Limits

Allow users to configure

- Daily limit
- Weekly limit
- Monthly limit
- Per-network limit

Warnings

- 75%
- 90%
- 100%

Actions

- Notify
- Block selected apps
- Block all traffic

---

## Deliverables

- Usage limits
- Notifications
- Automatic blocking

---

# Phase 7 — Domain Inspection

For every application maintain

- Domains
- Remote IPs
- Ports
- Protocols
- Bytes transferred

Example

```text
Firefox

github.com

api.github.com

objects.githubusercontent.com
```

---

## Deliverables

- Domain explorer
- Per-app domains

---

# Phase 8 — Reports

Generate reports

- Daily
- Weekly
- Monthly
- Custom

Charts

- Top applications
- Data usage
- Network usage

Export

- CSV
- JSON

Future

- PDF

---

## Deliverables

- Reports
- Export functionality

---

# Phase 9 — Automation

Scheduler

Examples

```text
08:00

↓

Enable Work Profile
```

CLI

```bash
traffic profile work

traffic block steam

traffic unblock firefox

traffic stats
```

Interfaces

- CLI
- D-Bus API
- REST API (future)

---

## Deliverables

- Scheduler
- CLI
- Public API

---

# Phase 10 — Polish

UI Improvements

- Floating bandwidth monitor
- Dark mode
- Light mode
- Live graphs
- Search
- Filters
- Notifications
- Keyboard shortcuts
- Multi-language support

---

# System Architecture

```text
                 ┌────────────────────────┐
                 │      GTK / Qt UI       │
                 └───────────┬────────────┘
                             │
                           D-Bus
                             │
                 ┌───────────▼────────────┐
                 │    Control Service     │
                 │ Profiles / Settings    │
                 └───────────┬────────────┘
                             │
        ┌────────────────────┼────────────────────┐
        │                    │                    │
┌───────▼────────┐   ┌────────▼────────┐   ┌──────▼────────┐
│ Connection     │   │ Firewall Engine │   │ Usage Storage │
│ Engine         │   │ nftables        │   │ SQLite        │
└────────────────┘   └─────────────────┘   └───────────────┘
```

---

# Design Principles

- Keep networking logic inside the daemon.
- Keep the UI stateless wherever possible.
- Make every feature build upon the previous phase.
- Store everything locally.
- Support automation through APIs.
- Design for extensibility from the beginning.
- Prefer stable Linux technologies (nftables, D-Bus, SQLite, NetworkManager).
- Minimise CPU and memory usage.
- Keep the project modular and testable.

---

# Future Ideas

- Bandwidth throttling
- VPN awareness
- Docker/container monitoring
- Virtual machine traffic
- DNS-over-HTTPS monitoring
- Prometheus metrics
- Grafana integration
- WireGuard/OpenVPN support
- AI-assisted traffic anomaly detection
- Remote monitoring dashboard
- Plugin system
