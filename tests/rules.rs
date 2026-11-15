//! Rules, messages and one end-to-end delivery.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

use plumber::daemon::Config;
use plumber::message::Message;
use plumber::rules::Rules;
use plumber::Daemon;

fn parse(text: &str) -> Rules {
    Rules::parse(text, &PathBuf::from(".")).expect("rules parse")
}

fn msg(text: &str) -> Message {
    Message::new("test", "text", text)
}

#[test]
fn the_wire_format_is_the_one_plumb6_fixes() {
    let message = Message {
        dst: "edit".into(),
        attr: vec![("addr".into(), ":42".into())],
        ..msg("a.rs:42")
    };
    let wire = message.to_wire();
    let fields: Vec<&str> = wire.split('\n').collect();
    assert_eq!(fields.len(), 7, "seven fields, data last: {wire:?}");
    assert_eq!(fields[0], "test");
    assert_eq!(fields[1], "edit");
    assert_eq!(fields[3], "text");
    assert_eq!(fields[4], "addr=:42");
    assert_eq!(fields[5], "7", "ndata is a byte count");
    assert_eq!(fields[6], "a.rs:42");
    // And it round-trips, including data with newlines, which is why ndata is a count.
    let multi = Message::new("test", "text", "line one\nline two");
    let back = Message::from_wire(&multi.to_wire()).expect("round trip");
    assert_eq!(back, multi);
}

#[test]
fn the_first_rule_set_that_matches_wins() {
    let rules = parse(
        "type is text\ndata matches 'hello'\nplumb to first\n\n\
         type is text\ndata matches 'hello world'\nplumb to second\n",
    );
    let route = rules.route(&msg("hello world"));
    assert_eq!(
        route.dst.as_deref(),
        Some("first"),
        "order in the file is the priority"
    );
}

#[test]
fn an_unmatched_message_is_discarded() {
    let rules = parse("type is text\ndata matches 'nothing matches this'\nplumb to edit\n");
    let route = rules.route(&msg("hello"));
    assert!(!route.matched(), "no rule set fired");
    assert_eq!(route.dst, None);
}

#[test]
fn a_path_that_exists_is_rewritten_to_its_absolute_form() {
    // The rule from plumbing.example, reduced: the file exists, so $file is set and the message
    // the receiver gets is the path, not the path plus a line number. A Windows path is written
    // with forward slashes here, which is how it appears in logs and in the example rules.
    // The rules regex cannot match a path with a space in it, and on a GitHub Windows runner
    // `temp_dir()` is `C:\Users\RUNNER~1\AppData\Local\Temp` — or worse, a real name with a
    // space. `CARGO_TARGET_TMPDIR` lives inside the checkout, so it is space-free everywhere.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("plumber-test-file");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, "hello").unwrap();
    let rules = parse(
        r#"type is text
data matches '((?:[a-zA-Z]:)?[a-zA-Z0-9_/\.\-]+)(:([0-9]+))?'
arg isfile $1
data set $file
attr add addr=$3
plumb to edit
"#,
    );
    let actual = file.to_string_lossy().replace('\\', "/");
    let sent = format!("{actual}:12");
    let route = rules.route(&msg(&sent));
    assert_eq!(
        route.dst.as_deref(),
        Some("edit"),
        "the path must resolve: {sent}"
    );
    // The resolved path may use either separator; compare as paths, not as strings.
    assert_eq!(
        std::path::Path::new(&route.message.data),
        file.as_path(),
        "data is the resolved file"
    );
    assert_eq!(
        route.message.attr_get("addr"),
        Some("12"),
        "capture 3 is the digits; capture 2 keeps the colon"
    );
}

#[test]
fn a_missing_file_falls_through_to_the_next_rule_set() {
    let rules = parse(
        "type is text\n\
         data matches '([a-zA-Z0-9_/\\.\\-]+)\\.rs'\n\
         arg isfile $1\n\
         plumb to edit\n\n\
         type is text\n\
         data matches '.*\\.rs'\n\
         plumb to stale-code\n",
    );
    let route = rules.route(&msg("nowhere/missing.rs"));
    assert_eq!(
        route.dst.as_deref(),
        Some("stale-code"),
        "isfile failed, so the next rule set ran"
    );
}

#[test]
fn macros_are_expanded_when_the_file_is_read() {
    let rules = parse("addr=':([0-9]+)'\ntype is text\ndata matches '^([a-z]+)'$addr\ndata set $1\nplumb to edit\n");
    let route = rules.route(&msg("main:42"));
    assert_eq!(route.dst.as_deref(), Some("edit"));
    assert_eq!(route.message.data, "main");
}

#[test]
fn a_rule_set_that_starts_with_an_action_is_refused() {
    let err = Rules::parse(
        "plumb to edit\n\ntype is text\ndata is x\nplumb to edit\n",
        &PathBuf::from("."),
    )
    .unwrap_err();
    assert!(err.contains("needs at least one pattern"), "{err}");
}

#[test]
fn a_bad_regular_expression_is_reported_with_its_line() {
    let err = Rules::parse(
        "type is text\ndata matches '([a-z'\nplumb to edit\n",
        &PathBuf::from("."),
    )
    .unwrap_err();
    assert!(err.contains("bad regular expression"), "{err}");
    assert!(err.contains("line 2"), "{err}");
}

#[test]
fn a_macro_inside_a_quoted_regular_expression_is_expanded() {
    // A `'` inside the quoted argument starts a new quoted span: reading the inner pair as literal
    // characters leaves a regex that matches nothing, and the rule set silently never fires.
    let rules = parse("addr=':([0-9]+)'\ntype is text\ndata matches '([a-z]+)('$addr')?'\nattr add addr=$2\nplumb to edit\n");
    let route = rules.route(&msg("main:42"));
    assert_eq!(route.dst.as_deref(), Some("edit"));
    assert_eq!(
        route.message.attr_get("addr"),
        Some(":42"),
        "the inner quotes are delimiters, not text"
    );
}

#[test]
fn a_macro_named_like_a_rule_variable_does_not_hijack_it() {
    // `$file` after `isfile` means "the resolved path". A macro called `file` must not replace it
    // with its own text — plumb(6)'s own example defines such a macro.
    let dir = std::env::temp_dir().join("plumber-test-macro-clash");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("note.txt");
    std::fs::write(&file, "hello").unwrap();
    let rules = parse("file='(hijacked)'\ntype is text\ndata matches '(.+)'\narg isfile $1\ndata set $file\nplumb to edit\n");
    let route = rules.route(&msg(&file.to_string_lossy()));
    assert_eq!(route.dst.as_deref(), Some("edit"));
    assert_ne!(
        route.message.data, "(hijacked)",
        "the variable wins over the macro"
    );
    assert_eq!(std::path::Path::new(&route.message.data), file.as_path());
}

#[test]
fn the_daemon_delivers_a_message_to_a_reader() {
    // A real socket, a real reader, a real message — no mocks.
    let dir = std::env::temp_dir().join("plumber-test-daemon");
    std::fs::create_dir_all(&dir).unwrap();
    let rules_path = dir.join("plumbing");
    std::fs::write(
        &rules_path,
        "type is text\ndata matches 'hello'\nattr add routed=yes\nplumb to greeting\n",
    )
    .unwrap();
    let config = Config {
        rules: rules_path,
        port: 0,
        port_file: None,
        allow_start: false,
    };
    let daemon = Daemon::bind(config).expect("bind");
    let port = daemon.local_port().expect("port");
    std::thread::spawn(move || {
        let _ = daemon.serve();
    });

    // The reader subscribes first, as in Plan 9 you must have the port open to receive a copy.
    let mut reader = TcpStream::connect(("127.0.0.1", port)).expect("connect reader");
    writeln!(reader, "READ greeting").unwrap();
    reader.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));

    let mut sender = TcpStream::connect(("127.0.0.1", port)).expect("connect sender");
    let wire = Message::new("test", "text", "hello world").to_wire();
    writeln!(sender, "SEND {}", wire.len()).unwrap();
    sender.write_all(wire.as_bytes()).unwrap();
    sender.flush().unwrap();
    let mut reply = String::new();
    BufReader::new(sender).read_line(&mut reply).unwrap();
    assert_eq!(
        reply.trim(),
        "OK greeting",
        "the sender is told where the message went"
    );

    // The reader gets the rewritten message.
    let mut reader = BufReader::new(reader);
    let mut length = String::new();
    reader.read_line(&mut length).unwrap();
    let bytes: usize = length.trim().parse().expect("frame length");
    let mut body = vec![0u8; bytes];
    reader.read_exact(&mut body).unwrap();
    let received = Message::from_wire(&String::from_utf8_lossy(&body)).expect("parse received");
    assert_eq!(received.data, "hello world");
    assert_eq!(received.dst, "greeting");
    assert_eq!(
        received.attr_get("routed"),
        Some("yes"),
        "the rule's attribute reached the reader"
    );
}

#[test]
fn a_message_for_a_port_nobody_reads_is_held_for_the_next_reader() {
    // plumb(4): when a rule names a client, the message waits instead of being refused. This is
    // what makes "send first, start the viewer later" work.
    let dir = std::env::temp_dir().join("plumber-test-hold");
    std::fs::create_dir_all(&dir).unwrap();
    let rules_path = dir.join("plumbing");
    std::fs::write(
        &rules_path,
        "type is text\ndata matches 'hello'\nattr add held=yes\nplumb to greeting\nplumb client greeting\n",
    )
    .unwrap();
    let daemon = Daemon::bind(Config {
        rules: rules_path,
        port: 0,
        port_file: None,
        allow_start: false,
    })
    .expect("bind");
    let port = daemon.local_port().expect("port");
    std::thread::spawn(move || {
        let _ = daemon.serve();
    });

    // Nobody is reading yet: the message is held, and the sender is told so.
    let mut sender = TcpStream::connect(("127.0.0.1", port)).expect("connect sender");
    let wire = Message::new("test", "text", "hello later").to_wire();
    writeln!(sender, "SEND {}", wire.len()).unwrap();
    sender.write_all(wire.as_bytes()).unwrap();
    sender.flush().unwrap();
    let mut reply = String::new();
    BufReader::new(sender).read_line(&mut reply).unwrap();
    assert_eq!(reply.trim(), "OK greeting (held for a client)");

    // A reader arrives afterwards and gets the message that was waiting.
    let mut reader =
        BufReader::new(TcpStream::connect(("127.0.0.1", port)).expect("connect reader"));
    reader.get_mut().write_all(b"READ greeting\n").unwrap();
    reader.get_mut().flush().unwrap();
    let mut length = String::new();
    reader.read_line(&mut length).unwrap();
    let bytes: usize = length.trim().parse().expect("frame length");
    let mut body = vec![0u8; bytes];
    reader.read_exact(&mut body).unwrap();
    let received = Message::from_wire(&String::from_utf8_lossy(&body)).expect("parse received");
    assert_eq!(received.data, "hello later");
    assert_eq!(received.attr_get("held"), Some("yes"));
}

#[test]
fn a_message_with_nowhere_to_go_is_an_error_to_the_sender() {
    let dir = std::env::temp_dir().join("plumber-test-noroute");
    std::fs::create_dir_all(&dir).unwrap();
    let rules_path = dir.join("plumbing");
    std::fs::write(
        &rules_path,
        "type is text\ndata matches 'nothing'\nplumb to edit\n",
    )
    .unwrap();
    let daemon = Daemon::bind(Config {
        rules: rules_path,
        port: 0,
        port_file: None,
        allow_start: false,
    })
    .expect("bind");
    let port = daemon.local_port().expect("port");
    std::thread::spawn(move || {
        let _ = daemon.serve();
    });
    let mut sender = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let wire = Message::new("test", "text", "unmatched text").to_wire();
    writeln!(sender, "SEND {}", wire.len()).unwrap();
    sender.write_all(wire.as_bytes()).unwrap();
    sender.flush().unwrap();
    let mut reply = String::new();
    BufReader::new(sender).read_line(&mut reply).unwrap();
    assert!(
        reply.trim().starts_with("ERR"),
        "Plan 9 gives the sender a write error: {reply:?}"
    );
}
