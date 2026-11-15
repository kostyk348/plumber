//! `plumberd` — the daemon.
//!
//! ```text
//! plumberd --rules plumbing [--port 5544] [--port-file ~/.plumber/port] [--allow-start]
//! ```
//!
//! `--port 0` (the default) lets the system choose a free port, which is then written to the port
//! file so `plumb` finds it without configuration. `--allow-start` permits `plumb start` rules to
//! run commands: a rules file is executable content, so that is opt-in.

use std::path::PathBuf;

use plumber::{Config, Daemon};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rules: Option<PathBuf> = None;
    let mut port: u16 = 0;
    let mut port_file: Option<PathBuf> = Some(default_port_file());
    let mut allow_start = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rules" => rules = it.next().map(PathBuf::from),
            "--port" => port = it.next().and_then(|p| p.parse().ok()).unwrap_or(0),
            "--port-file" => port_file = it.next().map(PathBuf::from),
            "--no-port-file" => port_file = None,
            "--allow-start" => allow_start = true,
            "--help" | "-h" => {
                println!("plumberd --rules FILE [--port N] [--port-file PATH] [--allow-start]");
                return;
            }
            other => {
                eprintln!("plumberd: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let Some(rules) = rules else {
        eprintln!("plumberd: --rules is required (a plumbing rules file)");
        std::process::exit(2);
    };
    let config = Config {
        rules,
        port,
        port_file,
        allow_start,
    };
    match Daemon::bind(config) {
        Ok(daemon) => {
            if let Err(e) = daemon.serve() {
                eprintln!("plumberd: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("plumberd: {e}");
            std::process::exit(1);
        }
    }
}

/// Where `plumb` looks for the port by default: next to the user's settings, not in the repo.
fn default_port_file() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("plumber").join("port");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".plumber").join("port")
}
