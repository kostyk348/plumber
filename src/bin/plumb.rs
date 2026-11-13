//! `plumb` — the client: send a message, read a port, or ask the rules what they would do.
//!
//! ```text
//! plumb send "src/app.rs:42" [--src log] [--type text] [--wdir DIR]
//! plumb read edit                # blocks; prints the messages that arrive
//! plumb rules                    # the rule set the daemon is running
//! plumb ports                    # which ports exist and how many readers each has
//! plumb test --rules plumbing "src/app.rs:42"   # dry run: which port, with what message
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use plumber::{Message, Rules};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first().cloned() else {
        usage();
        std::process::exit(2);
    };
    let rest = &args[1..];
    let result = match command.as_str() {
        "send" => send(rest),
        "read" => read(rest),
        "rules" => request(rest, "RULES"),
        "ports" => request(rest, "PORTS"),
        "test" => test(rest),
        "--help" | "-h" | "help" => {
            usage();
            Ok(())
        }
        other => Err(format!("unknown command {other:?}; try plumb --help")),
    };
    if let Err(e) = result {
        eprintln!("plumb: {e}");
        std::process::exit(1);
    }
}

fn usage() {
    println!(
        "plumb — send and receive plumber messages\n\n  \
         plumb send <text> [--src NAME] [--type text] [--wdir DIR] [--port N]\n  \
         plumb read <port> [--port N]\n  \
         plumb rules | plumb ports [--port N]\n  \
         plumb test --rules FILE <text> [--wdir DIR]   (dry run, no daemon)"
    );
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// The text of a command: every positional word, joined.
fn text_arg(args: &[String]) -> String {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(arg.clone());
    }
    out.join(" ")
}

fn port_of(args: &[String]) -> Result<u16, String> {
    if let Some(port) = flag(args, "--port") {
        return port.parse().map_err(|_| format!("bad --port {port:?}"));
    }
    let path = default_port_file();
    std::fs::read_to_string(&path)
        .map_err(|e| format!("no daemon port: {e} (start plumberd, or pass --port). Looked in {}", path.display()))?
        .trim()
        .parse()
        .map_err(|_| format!("{} does not contain a port number", path.display()))
}

fn connect(args: &[String]) -> Result<TcpStream, String> {
    let port = port_of(args)?;
    TcpStream::connect(("127.0.0.1", port)).map_err(|e| format!("cannot reach the plumber on 127.0.0.1:{port}: {e}"))
}

fn send(args: &[String]) -> Result<(), String> {
    let source = flag(args, "--src").unwrap_or("plumb");
    let kind = flag(args, "--type").unwrap_or("text");
    let wdir = flag(args, "--wdir").map(str::to_string).or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
    let text = text_arg(args);
    if text.is_empty() {
        return Err("send needs something to send".into());
    }
    let message = Message { wdir, ..Message::new(source, kind, &text) };
    let wire = message.to_wire();
    let mut stream = connect(args)?;
    write!(stream, "SEND {}\n", wire.len()).map_err(|e| e.to_string())?;
    stream.write_all(wire.as_bytes()).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).map_err(|e| e.to_string())?;
    let reply = reply.trim();
    if let Some(reason) = reply.strip_prefix("ERR ") {
        return Err(reason.to_string());
    }
    println!("{reply}");
    Ok(())
}

fn read(args: &[String]) -> Result<(), String> {
    let port_name = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .ok_or("read needs a port name")?;
    let mut stream = connect(args)?;
    write!(stream, "READ {port_name}\n").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut length = String::new();
        if reader.read_line(&mut length).map_err(|e| e.to_string())? == 0 {
            return Ok(()); // the daemon went away
        }
        let bytes: usize = length.trim().parse().map_err(|_| format!("bad frame length {length:?}"))?;
        let mut body = vec![0u8; bytes];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        match Message::from_wire(&String::from_utf8_lossy(&body)) {
            Ok(message) => println!("{}\t{}", message.attr_get("addr").unwrap_or("-"), message.data),
            Err(e) => eprintln!("plumb: unreadable message: {e}"),
        }
    }
}

fn request(args: &[String], command: &str) -> Result<(), String> {
    let mut stream = connect(args)?;
    writeln!(stream, "{command}").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let mut text = String::new();
    stream.read_to_string(&mut text).map_err(|e| e.to_string())?;
    print!("{text}");
    Ok(())
}

/// Dry run: what would the rules do with this text? No daemon, no delivery — the fastest way to
/// debug a plumbing file.
fn test(args: &[String]) -> Result<(), String> {
    let path = flag(args, "--rules").ok_or("test needs --rules FILE")?;
    let wdir = flag(args, "--wdir").map(str::to_string).or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned())).unwrap_or_default();
    let text = text_arg(args);
    let rules_path = PathBuf::from(path);
    let rules_text = std::fs::read_to_string(&rules_path).map_err(|e| format!("{path}: {e}"))?;
    let rules = Rules::parse(&rules_text, rules_path.parent().unwrap_or(std::path::Path::new(".")))?;
    let message = Message { wdir, ..Message::new("plumb-test", "text", &text) };
    let route = rules.route(&message);
    if !route.matched() {
        println!("no rule matched: this message would be discarded");
        return Ok(());
    }
    println!("rule: line {}", route.rule_line.unwrap_or(0));
    println!("port: {}", route.dst.clone().unwrap_or_else(|| "(none)".into()));
    if let Some(start) = &route.start {
        println!("start: {start}");
    }
    if let Some(client) = &route.client {
        println!("client: {client}");
    }
    println!("--- message the receiver gets ---");
    print!("{}", route.message.to_wire());
    println!();
    Ok(())
}

fn default_port_file() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("plumber").join("port");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".plumber").join("port")
}
