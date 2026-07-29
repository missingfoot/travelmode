//! travelmode: command-line client for travelmoded.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tokio::net::UnixStream;
use travelmode_core::ipc::{read_frame, write_frame, Request, Response};
use travelmode_core::types::*;

#[derive(Parser)]
#[command(name = "travelmode", version, about = "Per-application network control for Linux")]
struct Cli {
    /// Path to the daemon socket.
    #[arg(long, global = true, default_value = "/run/travelmode/daemon.sock")]
    socket: PathBuf,
    /// Machine-readable JSON output (raw response payload).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Daemon status.
    Status,
    /// Current network environment.
    Network,
    /// Processes holding network sockets.
    Ps,
    /// Tracked connections.
    Connections,
    /// Per-application usage.
    Top,
    /// List rules.
    Rules,
    /// Block an application (by exe path or name).
    Block {
        /// Executable path or bare binary name.
        target: String,
        /// Temporary rule: expire after this many seconds.
        #[arg(long)]
        temp: Option<u64>,
        /// Do not persist the rule across daemon restarts.
        #[arg(long)]
        no_persist: bool,
    },
    /// Allow an application (store an explicit allow rule).
    Allow {
        /// Executable path or bare binary name.
        target: String,
        /// Temporary rule: expire after this many seconds.
        #[arg(long)]
        temp: Option<u64>,
        /// Do not persist the rule across daemon restarts.
        #[arg(long)]
        no_persist: bool,
    },
    /// Remove a rule by id.
    Remove {
        /// Rule id (see `travelmode rules`).
        id: u64,
    },
    /// Pause all filtering.
    Pause,
    /// Resume filtering.
    Resume,
    /// Stream live events as JSON lines.
    Watch,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Die quietly on SIGPIPE like a normal Unix tool (Rust ignores
    // SIGPIPE by default, which turns `travelmode ps | head` into a
    // panic on println!).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("travelmode: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match &cli.command {
        Command::Block { target, temp, no_persist } => {
            let input = resolve_rule(&cli.socket, target, RuleAction::Block, *temp, *no_persist).await?;
            simple_request(&cli.socket, cli.json, Request::AddRule { rule: input }).await
        }
        Command::Allow { target, temp, no_persist } => {
            let input = resolve_rule(&cli.socket, target, RuleAction::Allow, *temp, *no_persist).await?;
            simple_request(&cli.socket, cli.json, Request::AddRule { rule: input }).await
        }
        Command::Remove { id } => {
            simple_request(&cli.socket, cli.json, Request::RemoveRule { id: *id }).await
        }
        Command::Pause => simple_request(&cli.socket, cli.json, Request::SetPaused { paused: true }).await,
        Command::Resume => simple_request(&cli.socket, cli.json, Request::SetPaused { paused: false }).await,
        Command::Watch => watch(&cli.socket).await,
        _ => {
            let request = match &cli.command {
                Command::Status => Request::GetStatus,
                Command::Network => Request::GetNetwork,
                Command::Ps => Request::GetProcesses,
                Command::Connections => Request::GetConnections,
                Command::Top => Request::GetTop,
                Command::Rules => Request::ListRules,
                _ => unreachable!(),
            };
            let response = roundtrip(&cli.socket, &request).await?;
            if cli.json {
                print_json(&response);
            } else {
                print_pretty(&response);
            }
            Ok(())
        }
    }
}

/// Send one request, read one response.
async fn roundtrip(socket: &PathBuf, request: &Request) -> Result<Response, String> {
    let mut stream = connect(socket).await?;
    write_frame(&mut stream, request)
        .await
        .map_err(|e| format!("failed to send request: {e}"))?;
    read_frame(&mut stream)
        .await
        .map_err(|e| format!("failed to read response: {e}"))
}

async fn connect(socket: &PathBuf) -> Result<UnixStream, String> {
    UnixStream::connect(socket).await.map_err(|e| {
        format!(
            "cannot connect to {}: {e} — is travelmoded running?",
            socket.display()
        )
    })
}

/// Send a request expecting Ok; prints the result unless --json.
async fn simple_request(socket: &PathBuf, json: bool, request: Request) -> Result<(), String> {
    let response = roundtrip(socket, &request).await?;
    if json {
        print_json(&response);
        return Ok(());
    }
    match response {
        Response::Ok => Ok(()),
        Response::Error { message } => Err(format!("daemon error: {message}")),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

fn print_json(response: &Response) {
    match serde_json::to_string_pretty(response) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("travelmode: cannot serialize response: {e}"),
    }
}

/// Resolve a block/allow target into a RuleInput: absolute path, PATH
/// lookup, or exact match against the daemon's process list.
async fn resolve_rule(
    socket: &PathBuf,
    target: &str,
    action: RuleAction,
    temp: Option<u64>,
    no_persist: bool,
) -> Result<RuleInput, String> {
    let exe_path = resolve_exe(socket, target).await?;
    let name = exe_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.to_string());
    Ok(RuleInput {
        name,
        exe_path,
        action,
        persistent: !no_persist && temp.is_none(),
        ttl_secs: temp,
    })
}

async fn resolve_exe(socket: &PathBuf, target: &str) -> Result<PathBuf, String> {
    // Explicit path.
    if target.contains('/') {
        let path = PathBuf::from(target);
        if path.exists() {
            return std::fs::canonicalize(&path)
                .map_err(|e| format!("cannot resolve {}: {e}", path.display()));
        }
        return Err(format!("no such file: {target}"));
    }
    // Bare name: PATH lookup (which-style).
    if let Some(path) = which(target) {
        return Ok(path);
    }
    // Fall back to an exact match in the daemon's process list.
    if let Ok(Response::Processes(procs)) = roundtrip(socket, &Request::GetProcesses).await {
        let mut matches = procs.iter().filter(|p| p.name == target);
        if let (Some(p), None) = (matches.next(), matches.next()) {
            if let Some(exe) = &p.exe {
                return Ok(exe.clone());
            }
        }
    }
    Err(format!(
        "cannot resolve '{target}' to an executable (not in PATH, not a unique running process)"
    ))
}

/// which-style lookup of an executable in PATH.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// Subscribe and print events as JSON lines until the daemon
/// disconnects or the user interrupts.
async fn watch(socket: &PathBuf) -> Result<(), String> {
    let mut stream = connect(socket).await?;
    write_frame(&mut stream, &Request::Subscribe)
        .await
        .map_err(|e| format!("failed to subscribe: {e}"))?;
    loop {
        match read_frame::<Response, _>(&mut stream).await {
            Ok(Response::Event(event)) => match serde_json::to_string(&event) {
                Ok(line) => println!("{line}"),
                Err(e) => eprintln!("travelmode: cannot serialize event: {e}"),
            },
            Ok(_) => {}
            Err(_) => return Ok(()), // daemon closed the connection
        }
    }
}

// --------------------------------------------------------- pretty output

fn print_pretty(response: &Response) {
    match response {
        Response::Status(s) => print_status(s),
        Response::Network(n) => print_network(n),
        Response::Processes(ps) => print_processes(ps),
        Response::Connections(cs) => print_connections(cs),
        Response::Top(apps) => print_top(apps),
        Response::Rules(rules) => print_rules(rules),
        Response::Error { message } => eprintln!("daemon error: {message}"),
        other => println!("{other:?}"),
    }
}

fn print_status(s: &Status) {
    println!("travelmoded {}", s.version);
    println!("  uptime:              {}s", s.uptime_secs);
    println!("  paused:              {}", yes_no(s.paused));
    println!(
        "  filtering:           {}",
        if s.filtering_active { "active" } else { "inactive" }
    );
    println!("  rules:               {}", s.rules_count);
    println!("  tracked connections: {}", s.tracked_connections);
    println!("  tracked processes:   {}", s.tracked_processes);
}

fn print_network(n: &NetworkInfo) {
    if let Some(ssid) = &n.ssid {
        println!("SSID:    {ssid}");
    }
    if let Some(metered) = n.metered {
        println!("Metered: {}", yes_no(metered));
    }
    println!(
        "Gateway: {}",
        n.gateway.map(|g| g.to_string()).unwrap_or_else(|| "-".into())
    );
    println!(
        "Primary: {}",
        n.primary_interface.clone().unwrap_or_else(|| "-".into())
    );
    println!(
        "DNS:     {}",
        if n.dns_servers.is_empty() {
            "-".to_string()
        } else {
            n.dns_servers
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!();
    let mut rows = vec![["NAME".into(), "KIND".into(), "STATE".into(), "MAC".into(), "ADDRESSES".into()]];
    for i in &n.interfaces {
        rows.push([
            i.name.clone(),
            format!("{:?}", i.kind).to_lowercase(),
            if i.is_up { "up".into() } else { "down".into() },
            i.mac.clone().unwrap_or_else(|| "-".into()),
            i.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "),
        ]);
    }
    print_table(&rows);
}

fn print_processes(ps: &[ProcessInfo]) {
    let mut rows = vec![["PID".into(), "NAME".into(), "USER".into(), "EXE".into()]];
    for p in ps {
        rows.push([
            p.pid.to_string(),
            p.name.clone(),
            p.user.clone().unwrap_or_else(|| "-".into()),
            p.exe.as_ref().map(|e| e.display().to_string()).unwrap_or_else(|| "-".into()),
        ]);
    }
    print_table(&rows);
}

fn print_connections(cs: &[ConnectionInfo]) {
    let mut rows = vec![[
        "PROCESS".into(),
        "PROTO".into(),
        "LOCAL".into(),
        "REMOTE".into(),
        "SENT".into(),
        "RECV".into(),
    ]];
    for c in cs {
        rows.push([
            c.process_name.clone().unwrap_or_else(|| "?".into()),
            format!("{:?}", c.protocol).to_lowercase(),
            format!("{}:{}", c.local_addr, c.local_port),
            format!("{}:{}", c.remote_addr, c.remote_port),
            human_bytes(c.bytes_sent),
            human_bytes(c.bytes_recv),
        ]);
    }
    print_table(&rows);
}

fn print_top(apps: &[AppUsage]) {
    let mut rows = vec![[
        "APP".into(),
        "SENT".into(),
        "RECV".into(),
        "CONNS".into(),
        "STATE".into(),
    ]];
    for a in apps {
        rows.push([
            a.name.clone(),
            human_bytes(a.bytes_sent),
            human_bytes(a.bytes_recv),
            a.connections.to_string(),
            if a.blocked { "blocked".into() } else { "allowed".into() },
        ]);
    }
    print_table(&rows);
}

fn print_rules(rules: &[Rule]) {
    let mut rows = vec![[
        "ID".into(),
        "NAME".into(),
        "ACTION".into(),
        "EXE".into(),
        "PERSIST".into(),
        "EXPIRES".into(),
    ]];
    for r in rules {
        rows.push([
            r.id.to_string(),
            r.name.clone(),
            format!("{:?}", r.action).to_lowercase(),
            r.exe_path.display().to_string(),
            yes_no(r.persistent).into(),
            r.expires_at
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".into()),
        ]);
    }
    print_table(&rows);
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Render a simple left-aligned table (first row is the header).
fn print_table<const N: usize>(rows: &[[String; N]]) {
    if rows.is_empty() {
        return;
    }
    let mut widths = [0usize; N];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    for (n, row) in rows.iter().enumerate() {
        let line = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                if i + 1 == N {
                    cell.clone() // no padding on the last column
                } else {
                    format!("{cell:<width$}", width = widths[i] + 2)
                }
            })
            .collect::<String>();
        println!("{}", line.trim_end());
        if n == 0 {
            let sep: String = widths.iter().map(|w| "-".repeat(w + 2)).collect();
            println!("{}", sep.trim_end());
        }
    }
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn resolves_nothing_for_missing_name() {
        assert!(which("definitely-not-a-real-binary-travelmode").is_none());
    }
}
