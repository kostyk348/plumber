//! The daemon: one loopback socket, a registry of ports, a rules file that is watched.
//!
//! Line protocol, deliberately small enough to speak from `nc` or a shell script:
//!
//! ```text
//! SEND <bytes>\n<wire message>      → OK <port>   |  ERR <reason>
//! READ <port>\n                     → a stream of <bytes>\n<wire message> blocks
//! PORTS\n                           → one "<port> <readers>" per line
//! RULES\n                           → the rules text the daemon is running
//! PING\n                            → PONG
//! ```
//!
//! `READ` blocks and holds the connection: that is what a reader program *is* in this model.

/// The largest message the daemon will read. A frame that claims a gigabyte should be refused,
/// not allocated: the socket is loopback-only, but "only I can reach it" is not a memory budget.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// How long one frame may take to arrive in full before the connection is treated as broken.
/// Generous for a loopback frame, short enough that a wedged client cannot hold a thread for long.
const FRAME_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The socket read timeout: just short enough to poll, so `read_frame` can enforce its own deadline
/// identically on every platform.
const FRAME_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// How long a connection may sit idle between commands before it is dropped. A client that keeps a
/// socket for later `SEND`s is legitimate; one that never says anything again is not worth a thread
/// forever, and five minutes is far longer than any human-paced use.
const CONNECTION_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::message::Message;
use crate::rules::Rules;

#[derive(Clone, Debug)]
pub struct Config {
    pub rules: PathBuf,
    /// Port to listen on; 0 asks the operating system for one.
    pub port: u16,
    /// Where to write the chosen port, so clients do not have to guess.
    pub port_file: Option<PathBuf>,
    /// Run `plumb start` commands. Off in tests: a rules file is executable content.
    pub allow_start: bool,
}

impl Config {
    pub fn new(rules: impl Into<PathBuf>) -> Self {
        Self {
            rules: rules.into(),
            port: 0,
            port_file: None,
            allow_start: false,
        }
    }
}

struct State {
    rules: Rules,
    rules_text: String,
    rules_mtime: Option<SystemTime>,
    /// The rules file this state was loaded from, kept for the modification-time check.
    path: PathBuf,
    /// Per port: the channels of the readers currently attached.
    readers: HashMap<String, Vec<Sender<Message>>>,
    /// Messages held for a port nobody is reading yet (`plumb client`).
    held: HashMap<String, Vec<Message>>,
}

impl State {
    fn reload_if_changed(&mut self) {
        let Ok(meta) = std::fs::metadata(self.rules_path()) else {
            return;
        };
        let modified = meta.modified().ok();
        if modified.is_some() && modified == self.rules_mtime {
            return;
        }
        let Ok(text) = std::fs::read_to_string(self.rules_path()) else {
            return;
        };
        if let Ok(rules) = Rules::parse(&text, self.rules_path().parent().unwrap_or(Path::new(".")))
        {
            self.rules = rules;
            self.rules_text = text;
            self.rules_mtime = modified;
        }
    }

    fn rules_path(&self) -> PathBuf {
        self.path.clone()
    }
}

pub struct Daemon {
    listener: TcpListener,
    state: Arc<Mutex<State>>,
    config: Config,
}

impl Daemon {
    /// Bind the socket and load the rules. The rules are validated here, so a broken file fails
    /// at startup with its line number instead of at the first message.
    pub fn bind(config: Config) -> Result<Self, String> {
        let text = std::fs::read_to_string(&config.rules)
            .map_err(|e| format!("{}: {e}", config.rules.display()))?;
        let base = config
            .rules
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let rules = Rules::parse(&text, &base)?;
        let mtime = std::fs::metadata(&config.rules)
            .ok()
            .and_then(|m| m.modified().ok());
        let mut readers = HashMap::new();
        for port in &rules.ports {
            readers.insert(port.clone(), Vec::new());
        }
        let state = State {
            rules,
            rules_text: text,
            rules_mtime: mtime,
            readers,
            held: HashMap::new(),
            path: config.rules.clone(),
        };
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .map_err(|e| format!("cannot listen: {e}"))?;
        Ok(Self {
            listener,
            state: Arc::new(Mutex::new(state)),
            config,
        })
    }

    pub fn local_port(&self) -> Result<u16, String> {
        self.listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| e.to_string())
    }

    /// Serve until the process is stopped. One thread per client: a reader holds its connection
    /// open for as long as it is running, and a sender is short-lived.
    pub fn serve(self) -> Result<(), String> {
        let port = self.local_port()?;
        println!("plumberd listening on 127.0.0.1:{port}");
        println!("rules: {}", self.config.rules.display());
        if let Some(path) = &self.config.port_file {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, port.to_string())
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else { continue };
            let state = Arc::clone(&self.state);
            let allow_start = self.config.allow_start;
            std::thread::spawn(move || {
                let _ = handle(stream, state, allow_start);
            });
        }
        Ok(())
    }
}

/// Read one command line, tolerating idle time between commands.
///
/// `read_line` cannot be used directly: with a socket poll timeout it reports `WouldBlock` on an
/// idle-but-healthy connection, and a partial line would already be appended to the buffer. Reading
/// through `fill_buf` keeps that decision explicit — EOF is `None`, a timeout past
/// `CONNECTION_IDLE_TIMEOUT` is an error, and anything else just waits.
fn read_command(reader: &mut impl BufRead) -> Result<Option<String>, String> {
    let deadline = std::time::Instant::now() + CONNECTION_IDLE_TIMEOUT;
    let mut line = Vec::new();
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err("connection idle for too long".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        };
        if chunk.is_empty() {
            return Ok(None); // the client hung up
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(end) => {
                line.extend_from_slice(&chunk[..end]);
                reader.consume(end + 1);
                let text = String::from_utf8_lossy(&line)
                    .trim_end_matches('\r')
                    .to_string();
                return Ok(Some(text));
            }
            None => {
                let len = chunk.len();
                line.extend_from_slice(chunk);
                reader.consume(len);
            }
        }
    }
}

/// Read exactly `n` bytes of a frame, or fail with a message that says what happened.
///
/// This is a hand-rolled loop rather than `read_exact` because a read timeout means different
/// things on different systems: under BSD/macOS a socket timeout can surface as a short read or as
/// `WouldBlock`, and the original `read_exact` turned both into a bare EOF that the client saw as
/// "the daemon hung up". Here the timeout is a sleep-wait poll with our own deadline, so every
/// platform produces the same error string, and a closed peer is reported as such.
fn read_frame(reader: &mut impl BufRead, n: usize) -> Result<Vec<u8>, String> {
    let deadline = std::time::Instant::now() + FRAME_READ_TIMEOUT;
    let mut body = vec![0u8; n];
    let mut filled = 0;
    while filled < n {
        match reader.read(&mut body[filled..]) {
            Ok(0) => {
                return Err(format!(
                    "connection closed after {filled} of {n} bytes of message"
                ))
            }
            Ok(read) => filled += read,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "timed out after {}s waiting for {n} bytes of message",
                        FRAME_READ_TIMEOUT.as_secs()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("short message: {e}")),
        }
    }
    Ok(body)
}

fn handle(stream: TcpStream, state: Arc<Mutex<State>>, allow_start: bool) -> Result<(), String> {
    // A client that says "SEND 4096" and then stops sending must not pin this thread: the socket
    // timeout is the poll interval, and `read_frame` owns the real deadline.
    let _ = stream.set_read_timeout(Some(FRAME_POLL));
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut writer = stream;
    loop {
        let Some(command) = read_command(&mut reader)? else {
            return Ok(()); // client hung up
        };
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["SEND", count] => {
                // Anything the sender got wrong must come back as an error on this connection:
                // a client that is silently disconnected cannot tell a bug from a crash, and this
                // is the one place where the protocol is wide enough to get it wrong.
                let body: Result<String, String> = (|| {
                    let bytes: usize = count
                        .parse()
                        .map_err(|_| format!("SEND needs a byte count, got {count:?}"))?;
                    if bytes > MAX_MESSAGE_BYTES {
                        return Err(format!(
                            "message of {bytes} bytes is larger than the {MAX_MESSAGE_BYTES}-byte limit"
                        ));
                    }
                    let body = read_frame(&mut reader, bytes)?;
                    Ok(String::from_utf8_lossy(&body).into_owned())
                })();
                let reply = match body {
                    Ok(text) => send(&state, &text, allow_start),
                    Err(e) => format!("ERR malformed message: {e}"),
                };
                writeln!(writer, "{reply}").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
            }
            ["READ", port] => {
                let port = port.to_string();
                let (sender, receiver) = channel::<Message>();
                {
                    let mut state = state.lock().map_err(|e| e.to_string())?;
                    state.readers.entry(port.clone()).or_default().push(sender);
                    // A held message (a `plumb client` waiting for its reader) is delivered now.
                    if let Some(held) = state.held.remove(&port) {
                        if let Some(last) = state.readers.get(&port).and_then(|list| list.last()) {
                            for message in held {
                                let _ = last.send(message);
                            }
                        }
                    }
                }
                for message in receiver {
                    let wire = message.to_wire();
                    if writeln!(writer, "{}", wire.len())
                        .and_then(|()| writer.write_all(wire.as_bytes()))
                        .is_err()
                    {
                        break;
                    }
                    if writer.flush().is_err() {
                        break;
                    }
                }
                return Ok(());
            }
            ["PORTS"] => {
                let state = state.lock().map_err(|e| e.to_string())?;
                let mut lines = String::new();
                for (port, readers) in &state.readers {
                    lines.push_str(&format!("{port} {}\n", readers.len()));
                }
                write!(writer, "{lines}").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                // A one-shot query: closing the connection is what tells the client the answer is
                // complete. Leaving it open makes `read_to_string` wait forever.
                return Ok(());
            }
            ["RULES"] => {
                let state = state.lock().map_err(|e| e.to_string())?;
                writer
                    .write_all(state.rules_text.as_bytes())
                    .map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                return Ok(());
            }
            ["PING"] => {
                writeln!(writer, "PONG").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
            }
            _ => {
                writeln!(
                    writer,
                    "ERR unknown command (SEND n | READ port | PORTS | RULES | PING)"
                )
                .map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
            }
        }
    }
}

/// Route one message and deliver it. The reply is what the sender sees: the port it went to, or
/// why it went nowhere — in Plan 9 a write to `send` fails when no rule and no reader exists.
fn send(state: &Arc<Mutex<State>>, wire: &str, allow_start: bool) -> String {
    let message = match Message::from_wire(wire) {
        Ok(message) => message,
        Err(e) => return format!("ERR {e}"),
    };
    let mut state = match state.lock() {
        Ok(state) => state,
        Err(e) => return format!("ERR {e}"),
    };
    state.reload_if_changed();
    let route = state.rules.route(&message);
    if !route.matched() {
        // plumb(6): no rule set fired, so the message is discarded unless it names an existing port.
        let named = route.message.dst.clone();
        if !named.is_empty() && state.readers.contains_key(&named) {
            deliver(&mut state, &named, route.message);
            return format!("OK {named}");
        }
        return "ERR no rule matched this message".to_string();
    }
    let delivered_to = route.dst.clone();
    if let Some(command) = route.start.clone() {
        if allow_start {
            spawn(&command);
        }
        // `plumb start` runs the command and discards the message by definition.
        return match delivered_to {
            Some(port) => format!("OK {port} (start)"),
            None => "OK (start)".to_string(),
        };
    }
    match delivered_to {
        Some(port) => {
            if state
                .readers
                .get(&port)
                .map(|r| r.is_empty())
                .unwrap_or(true)
            {
                if let Some(command) = route.client.clone() {
                    if allow_start {
                        spawn(&command);
                    }
                    // Hold the message for whoever opens the port next.
                    state
                        .held
                        .entry(port.clone())
                        .or_default()
                        .push(route.message.clone());
                    return format!("OK {port} (held for a client)");
                }
                // Nobody reading and nothing to start: discarded, as in Plan 9.
                return format!(
                    "ERR no reader on port {port} and the rule names no client or start"
                );
            }
            deliver(&mut state, &port, route.message);
            format!("OK {port}")
        }
        None => "ERR the rule set has no `plumb to`".to_string(),
    }
}

fn deliver(state: &mut State, port: &str, message: Message) {
    let list = state.readers.entry(port.to_string()).or_default();
    // A reader whose connection died is dropped: its channel is closed.
    list.retain(|sender| sender.send(message.clone()).is_ok());
}

fn spawn(command: &str) {
    // Split on spaces only for the executable; the rest goes as arguments, unquoted. A rules file
    // is executable content, which is why `Config::allow_start` exists and defaults to off.
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else { return };
    let _ = Command::new(program).args(parts).spawn();
}
