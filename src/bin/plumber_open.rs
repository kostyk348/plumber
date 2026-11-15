//! `plumber-open` — a reader that actually opens what a message names.
//!
//! ```text
//! plumber-open edit                        # a reader for the `edit` port
//! plumber-open web      --cmd 'firefox {data}'
//! plumber-open pdf      --once --dry-run   # show the command line, open nothing
//! ```
//!
//! The logic lives in [`plumber::open`] so it can be tested without starting a program.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use plumber::message::Message;
use plumber::open;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        usage();
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("plumber-open: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    println!(
        "plumber-open <port> [--cmd 'TEMPLATE'] [--port N] [--dry-run] [--once]\n\n\
         ports with a default opener: edit, web, pdf, scene\n\
         placeholders: {{data}} {{file}} {{line}} {{page}} {{addr}} {{dir}} {{attr:name}}\n\n\
         example: plumber-open edit --cmd 'notepad {{file}}'"
    );
}

fn run(args: &[String]) -> Result<(), String> {
    let port_name = args[0].clone();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let once = args.iter().any(|a| a == "--once");
    let template = flag(args, "--cmd").map(str::to_string);

    let mut stream = connect(args)?;
    writeln!(stream, "READ {port_name}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut length = String::new();
        if reader.read_line(&mut length).map_err(|e| e.to_string())? == 0 {
            return Ok(()); // the daemon went away
        }
        let bytes: usize = length
            .trim()
            .parse()
            .map_err(|_| format!("bad frame length {length:?}"))?;
        let mut body = vec![0u8; bytes];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        let message = Message::from_wire(&String::from_utf8_lossy(&body))?;
        match open::open(&message, &port_name, template.as_deref(), dry_run) {
            Ok(command) => println!("{command}"),
            Err(e) => eprintln!("plumber-open: {}: {e}", message.data),
        }
        if once {
            return Ok(());
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn connect(args: &[String]) -> Result<TcpStream, String> {
    let port: u16 = match flag(args, "--port") {
        Some(port) => port.parse().map_err(|_| format!("bad --port {port:?}"))?,
        None => std::fs::read_to_string(port_file())
            .map_err(|e| format!("no daemon port: {e} (start plumberd, or pass --port)"))?
            .trim()
            .parse()
            .map_err(|_| "the port file does not contain a number".to_string())?,
    };
    TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("cannot reach the plumber on 127.0.0.1:{port}: {e}"))
}

fn port_file() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("plumber").join("port");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".plumber").join("port")
}
