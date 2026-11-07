//! ptcc-hub command line. Run on the machine that hosts the hub; the desktop app never bundles it.
use ptcc_hub::{api, private_bind, serve, store};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "ptcc-hub — self-hosted workspace hub for PAPERTRAIL // COMMAND CENTER

USAGE
  ptcc-hub serve [--data DIR] [--port 8770] [--bind IP]... [--allow-public]
  ptcc-hub new-workspace NAME [--data DIR]     create a workspace and print a one-time owner invitation
  ptcc-hub backup [--data DIR]                 write a dated backup now
  ptcc-hub restore FILE [--data DIR]           replace the database with a backup (stop serve first)
  ptcc-hub revoke-user USER_ID [--data DIR]    revoke a user and every device they have

Default data folder: %LOCALAPPDATA%\\ptcc-hub. By default serve binds 127.0.0.1 and this PC's Tailscale address only;
any other address needs --allow-public (not recommended).";

fn main() {
    if let Err(e) = run(std::env::args().skip(1).collect()) {
        eprintln!("ptcc-hub: {e}");
        std::process::exit(1);
    }
}

fn take(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn run(mut args: Vec<String>) -> Result<(), String> {
    let data = take(&mut args, "--data")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("ptcc-hub"));
    let cmd = if args.is_empty() { String::new() } else { args.remove(0) };
    match cmd.as_str() {
        "serve" => serve_cmd(&data, args),
        "new-workspace" => {
            let name = args.join(" ");
            if name.trim().is_empty() {
                return Err("new-workspace needs a NAME".into());
            }
            let c = store::open(Some(&data))?;
            let (ws, token) = api::new_workspace(&c, name.trim())?;
            let host = tailscale_ip().map_or("127.0.0.1".to_string(), |ip| ip.to_string());
            println!("Workspace {ws} created.\nOwner invite link (single use, 72 h, shown once). Paste it into the Command Center:\n\n  http://{host}:8770#{token}\n\n(Change the port if you serve on another one.)");
            Ok(())
        }
        "backup" => {
            let c = store::open(Some(&data))?;
            let p = store::backup(&c, &store::backups_dir(&data), &format!("manual-{}.db", store::now().replace(':', "")))?;
            println!("Backup written: {}", p.display());
            Ok(())
        }
        "restore" => {
            let from = args.first().ok_or("restore needs a FILE")?;
            let saved = store::restore(&data, Path::new(from))?;
            println!("Restored {from}. The previous database was saved to {}", saved.display());
            Ok(())
        }
        "revoke-user" => {
            let user = args.first().ok_or("revoke-user needs a USER_ID")?;
            let c = store::open(Some(&data))?;
            if !api::revoke_user(&c, user)? {
                return Err(format!("no user {user}"));
            }
            println!("Revoked {user} and all their devices.");
            Ok(())
        }
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

fn tailscale_ip() -> Option<IpAddr> {
    for exe in ["tailscale", r"C:\Program Files\Tailscale\tailscale.exe"] {
        if let Ok(o) = std::process::Command::new(exe).args(["ip", "-4"]).output() {
            if let Some(ip) = String::from_utf8_lossy(&o.stdout).lines().find_map(|l| l.trim().parse().ok()) {
                return Some(ip);
            }
        }
    }
    None
}

fn serve_cmd(data: &Path, mut args: Vec<String>) -> Result<(), String> {
    let allow_public = args.iter().any(|a| a == "--allow-public");
    args.retain(|a| a != "--allow-public");
    let port: u16 = take(&mut args, "--port").map(|p| p.parse().map_err(|_| "bad --port")).transpose()?.unwrap_or(8770);
    let mut binds: Vec<IpAddr> = Vec::new();
    while let Some(b) = take(&mut args, "--bind") {
        binds.push(b.parse().map_err(|_| format!("bad --bind {b}"))?);
    }
    if binds.is_empty() {
        binds.push(IpAddr::from([127, 0, 0, 1]));
        match tailscale_ip() {
            Some(ip) => binds.push(ip),
            None => eprintln!("ptcc-hub: Tailscale not found or not signed in; serving on loopback only"),
        }
    }
    if let Some(b) = binds.iter().find(|b| !private_bind(**b)) {
        if !allow_public {
            return Err(format!("{b} is not a loopback or Tailscale address; pass --allow-public to bind it anyway (not recommended)"));
        }
        eprintln!("ptcc-hub: WARNING binding public address {b}");
    }
    let _lock = store::lock(data)?;
    let hub = Arc::new(api::Hub::new(store::open(Some(data))?, Some(data.join("media"))));
    let mut workers = Vec::new();
    for ip in &binds {
        let addr = std::net::SocketAddr::new(*ip, port);
        let server = Arc::new(tiny_http::Server::http(addr).map_err(|e| format!("can't listen on {addr}: {e}"))?);
        println!("ptcc-hub listening on http://{addr}");
        workers.extend(serve(hub.clone(), server, 4));
    }
    // Nightly backup: checked hourly, written once per UTC day, rotated (§10).
    let backups = store::backups_dir(data);
    loop {
        if let Ok(c) = hub.db.lock() {
            if let Err(e) = store::prune_ops(&c) {
                eprintln!("ptcc-hub: pruning applied ops failed: {e}");
            }
            match store::daily_backup(&c, &backups) {
                Ok(Some(p)) => println!("ptcc-hub: backup {}", p.display()),
                Ok(None) => {}
                Err(e) => eprintln!("ptcc-hub: backup failed: {e}"),
            }
        }
        std::thread::sleep(Duration::from_secs(3600));
    }
}
