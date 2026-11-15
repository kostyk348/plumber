//! End-to-end tests against the `plumberd` binary itself.
//!
//! The library tests run the daemon in-process; these start the real executable,
//! let it pick a port and write the port file, and then speak the line protocol
//! over a real socket — the same path `plumb` and `plumber-open` take.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use plumber::Message;

/// A daemon process that is killed when the test ends, even on panic.
struct DaemonProcess {
    child: Child,
    dir: PathBuf,
}

impl DaemonProcess {
    fn start(rules: &str) -> Self {
        let dir = unique_dir();
        std::fs::create_dir_all(&dir).expect("temp dir");
        let rules_path = dir.join("plumbing");
        std::fs::write(&rules_path, rules).expect("write rules");

        // stderr goes to a file rather than a pipe: a panicking daemon must not be able to fill a
        // pipe and block, and a failing assertion can quote what the daemon said before it died.
        let stderr = std::fs::File::create(dir.join("stderr.log")).expect("stderr log");
        let child = Command::new(env!("CARGO_BIN_EXE_plumberd"))
            .arg("--rules")
            .arg(&rules_path)
            .arg("--port-file")
            .arg(dir.join("port"))
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn plumberd");
        let mut process = Self { child, dir };
        process.wait_for_port();
        process
    }

    fn port_file(&self) -> PathBuf {
        self.dir.join("port")
    }

    fn wait_for_port(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(port) = read_port(&self.port_file()) {
                if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "plumberd did not publish a reachable port in {:?} ({}); stderr: {}",
            self.dir,
            if self.child.try_wait().ok().flatten().is_some() {
                "the process has exited"
            } else {
                "the process is still running"
            },
            self.stderr_tail()
        );
    }

    fn port(&self) -> u16 {
        read_port(&self.port_file()).expect("port file")
    }

    fn rules_path(&self) -> PathBuf {
        self.dir.join("plumbing")
    }

    /// What the daemon wrote to stderr so far — a panicking daemon explains itself here, and a
    /// silent EOF from the socket is otherwise indistinguishable from a crash.
    fn stderr_tail(&self) -> String {
        let text = std::fs::read_to_string(self.dir.join("stderr.log")).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(6).collect();
        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
    }

    /// Alive means "answers PING on a fresh connection".
    fn alive(&self) -> bool {
        self.best_effort_request("PING").as_deref() == Some("PONG")
    }

    fn best_effort_request(&self, line: &str) -> Option<String> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        writeln!(stream, "{line}").ok()?;
        stream.flush().ok()?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).ok()?;
        Some(reply.trim_end().to_string())
    }

    /// One request line in, one reply line out, on a fresh connection.
    fn request(&self, line: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect");
        writeln!(stream, "{line}").expect("write request");
        stream.flush().expect("flush");
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .expect("read reply");
        reply.trim_end().to_string()
    }

    /// One request line in, everything the daemon writes back (until it closes).
    fn request_all(&self, line: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect");
        writeln!(stream, "{line}").expect("write request");
        stream.flush().expect("flush");
        let mut body = String::new();
        stream.read_to_string(&mut body).expect("read reply");
        body
    }

    /// Send raw bytes and read whatever the daemon answers (a misbehaving client's view).
    fn raw(&self, payload: &[u8]) -> String {
        let reply = self.raw_quiet(payload);
        if reply.is_empty() {
            // An empty line means the daemon closed without answering. Say whether it is still
            // healthy and quote its stderr, so the next reader of this failure has the reason
            // instead of guessing.
            let why = if self.alive() {
                "daemon is still healthy (PING answered)".to_string()
            } else {
                "daemon is gone".to_string()
            };
            panic!(
                "no answer to a raw frame: {why}; stderr: {}",
                self.stderr_tail()
            );
        }
        reply
    }

    fn raw_quiet(&self, payload: &[u8]) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect");
        stream.write_all(payload).expect("write raw");
        stream.flush().expect("flush");
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .expect("read reply");
        reply.trim_end().to_string()
    }

    /// Send a partial frame and close our write side: the daemon must see EOF and still answer.
    /// This is the platform-independent version of "the client went away mid-frame" — the case that
    /// surfaced as a silent hang-up on macOS.
    fn raw_half_closed(&self, payload: &[u8]) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect");
        stream.write_all(payload).expect("write raw");
        stream.flush().expect("flush");
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("half-close");
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .expect("read reply");
        reply.trim_end().to_string()
    }

    /// SEND a message and read the daemon's verdict.
    fn send(&self, message: &Message) -> String {
        let wire = message.to_wire();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect");
        writeln!(stream, "SEND {}", wire.len()).expect("write length");
        stream.write_all(wire.as_bytes()).expect("write body");
        stream.flush().expect("flush");
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .expect("read verdict");
        reply.trim_end().to_string()
    }

    /// Subscribe to a port; the returned reader blocks on `read_message`.
    fn reader(&self, port: &str) -> BufReader<TcpStream> {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port())).expect("connect reader");
        writeln!(stream, "READ {port}").expect("write READ");
        stream.flush().expect("flush");
        // Give the daemon a moment to register the reader before anything is sent.
        std::thread::sleep(Duration::from_millis(120));
        BufReader::new(stream)
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn unique_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("plumber-e2e-{}-{nanos}", std::process::id()))
}

fn read_port(path: &Path) -> Option<u16> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_message(reader: &mut BufReader<TcpStream>) -> Message {
    let mut length = String::new();
    reader.read_line(&mut length).expect("frame length");
    let bytes: usize = length.trim().parse().expect("frame length number");
    let mut body = vec![0u8; bytes];
    reader.read_exact(&mut body).expect("frame body");
    Message::from_wire(&String::from_utf8_lossy(&body)).expect("message parses")
}

const EDIT_RULE: &str =
    "type is text\ndata matches '([a-zA-Z0-9_/\\-]+):([0-9]+)'\nattr add line=$2\nplumb to edit\n";

#[test]
fn daemon_binary_answers_ping_and_serves_its_rules() {
    let daemon = DaemonProcess::start(EDIT_RULE);

    assert_eq!(daemon.request("PING"), "PONG");
    let rules = daemon.request_all("RULES");
    assert!(
        rules.contains("plumb to edit"),
        "RULES returns the text it is running, got {rules:?}"
    );
    // Ports named in the rules are known from the start, with no readers yet.
    let ports = daemon.request_all("PORTS");
    assert!(
        ports.lines().all(|line| line.ends_with(" 0")),
        "nothing is subscribed yet, got {ports:?}"
    );
    assert!(
        ports.contains("edit"),
        "the port from the rules is listed, got {ports:?}"
    );
}

#[test]
fn daemon_binary_routes_a_message_to_a_reader() {
    let daemon = DaemonProcess::start(EDIT_RULE);
    let mut reader = daemon.reader("edit");

    let verdict = daemon.send(&Message::new("test", "text", "src/main.rs:42"));
    assert_eq!(verdict, "OK edit");

    let received = read_message(&mut reader);
    assert_eq!(received.data, "src/main.rs:42");
    assert_eq!(received.dst, "edit");
    assert_eq!(
        received.attr_get("line"),
        Some("42"),
        "the capture travelled as an attribute"
    );
    assert!(
        daemon.request("PORTS").starts_with("edit 1"),
        "PORTS counts the attached reader"
    );
}

#[test]
fn daemon_binary_explains_why_a_message_did_not_land() {
    let daemon = DaemonProcess::start(EDIT_RULE);

    let unmatched = daemon.send(&Message::new("test", "text", "no address here"));
    assert_eq!(unmatched, "ERR no rule matched this message");

    // The rule matches, but nobody reads the port and it is not a client — the sender is told.
    let no_reader = daemon.send(&Message::new("test", "text", "src/main.rs:7"));
    assert!(
        no_reader.starts_with("ERR no reader on port edit"),
        "got {no_reader:?}"
    );
}

#[test]
fn daemon_binary_picks_up_a_changed_rules_file() {
    let daemon = DaemonProcess::start(EDIT_RULE);
    // Fresh rules: the same text now belongs to `notes`.
    std::thread::sleep(Duration::from_millis(1100)); // mtime granularity
    std::fs::write(
        daemon.rules_path(),
        "type is text\ndata matches 'main'\nplumb to notes\n",
    )
    .expect("rewrite rules");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut verdict = String::new();
    while Instant::now() < deadline {
        verdict = daemon.send(&Message::new("test", "text", "src/main.rs:1"));
        if verdict.starts_with("OK notes") || verdict.starts_with("ERR no reader on port notes") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        verdict.contains("notes"),
        "rules were not reloaded, last verdict {verdict:?}"
    );
}

#[test]
fn daemon_binary_rejects_an_unknown_command() {
    let daemon = DaemonProcess::start(EDIT_RULE);
    let reply = daemon.request("FROBNICATE");
    assert!(reply.starts_with("ERR unknown command"), "got {reply:?}");
}

#[test]
fn daemon_binary_answers_malformed_messages_instead_of_hanging_up() {
    let daemon = DaemonProcess::start(EDIT_RULE);

    // A frame shorter than it claims: the sender must be told, not disconnected in silence.
    let short = daemon.raw(b"SEND 40\nonly a few bytes");
    assert!(
        short.starts_with("ERR malformed message"),
        "short frame: {short:?}"
    );

    // A byte count that is not a number at all.
    let not_a_number = daemon.raw(b"SEND lots\n");
    assert!(
        not_a_number.starts_with("ERR malformed message"),
        "bad count: {not_a_number:?}"
    );

    // A frame that lies about ndata (says 7, carries 13 bytes of UTF-8). It must be answered —
    // not silently dropped, and above all not a panic inside the connection thread.
    let body = "whoami\nedit\n/tmp\ntext\n\n7\nоткуда!".as_bytes();
    let mut payload = format!("SEND {}\n", body.len()).into_bytes();
    payload.extend_from_slice(body);
    let lying = daemon.raw(&payload);
    assert!(
        lying.starts_with("OK ") || lying.starts_with("ERR "),
        "lying ndata must still be answered, got {lying:?}"
    );

    // And after all that the daemon is still healthy.
    assert_eq!(daemon.request("PING"), "PONG");
}

#[test]
fn daemon_binary_answers_a_frame_cut_short_by_the_client() {
    // "SEND 40" followed by 16 bytes and a closed socket: the daemon has to name the problem
    // instead of dropping the connection, which is what a client cannot tell from a crash.
    let daemon = DaemonProcess::start(EDIT_RULE);
    let reply = daemon.raw_half_closed(b"SEND 40\nonly a few bytes");
    assert!(
        reply.starts_with("ERR malformed message") && reply.contains("closed"),
        "got {reply:?}"
    );
    assert_eq!(daemon.request("PING"), "PONG");
}

#[test]
fn daemon_binary_refuses_an_absurd_message_size() {
    let daemon = DaemonProcess::start(EDIT_RULE);
    // 4 GiB would be allocated before any parsing happens if the limit were missing.
    let reply = daemon.raw(b"SEND 4294967296\n");
    assert!(
        reply.starts_with("ERR malformed message") && reply.contains("limit"),
        "got {reply:?}"
    );
    assert_eq!(daemon.request("PING"), "PONG");
}
