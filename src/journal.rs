//! The daemon's journal: what it received, where it went, and what it answered.
//!
//! Plan 9's plumber had nothing like this — you watched the world with `strace` or by adding a
//! reader to every port. A small ring buffer is the difference between "the rules are a black box"
//! and a tool you can operate: the GUI tail, `plumb log --follow` and the tests all read this.
//!
//! Entries are serialised as JSON lines by hand: the crate has no dependencies, and a journal is
//! not a reason to gain one. Each line is a flat object with a fixed set of fields.

use std::collections::VecDeque;

/// How many entries the daemon remembers. Enough for "what just happened" on a busy machine
/// (a message a second for eight minutes), small enough to cost nothing.
pub const JOURNAL_CAPACITY: usize = 500;

/// One message as the daemon saw it.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Monotonic sequence number, so a client can ask for "everything after 42".
    pub seq: u64,
    /// Seconds since the Unix epoch (fractional). The GUI formats it; the protocol keeps it raw.
    pub at: f64,
    pub src: String,
    pub dst: String,
    /// The data, truncated: the journal is for watching, not for archiving.
    pub data: String,
    /// Attributes as `k=v` pairs, joined by spaces.
    pub attr: String,
    pub kind: String,
    /// The rule set line that fired, if any.
    pub rule: Option<usize>,
    /// The verdict exactly as it was sent back to the client: `OK edit`, `ERR no rule matched …`.
    pub verdict: String,
}

impl Entry {
    /// Truncate to `max` characters, marking the cut. Counts characters, not bytes: a UTF-8
    /// boundary bug here would be invisible until somebody sent Russian text.
    pub fn truncate(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            return text.to_string();
        }
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }

    /// One JSON object, no trailing newline.
    pub fn to_json(&self) -> String {
        let rule = match self.rule {
            Some(line) => format!("{line}"),
            None => "null".to_string(),
        };
        format!(
            "{{\"seq\":{},\"at\":{:.3},\"src\":{},\"dst\":{},\"type\":{},\"attr\":{},\"rule\":{},\"verdict\":{},\"data\":{}}}",
            self.seq,
            self.at,
            json_string(&self.src),
            json_string(&self.dst),
            json_string(&self.kind),
            json_string(&self.attr),
            rule,
            json_string(&self.verdict),
            json_string(&self.data),
        )
    }
}

/// Escape a string as a JSON string literal.
///
/// Hand-rolled on purpose: the alternative is a dependency for twenty lines of escaping. Control
/// characters become `\u00XX`, which is what a JSON parser expects, and invalid UTF-16 surrogates
/// cannot appear because the input is `&str`.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The ring buffer itself.
#[derive(Debug, Default)]
pub struct Journal {
    entries: VecDeque<Entry>,
    next_seq: u64,
}

impl Journal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the sequence number for a message that is about to be written down.
    pub fn next_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    /// Record an entry, dropping the oldest when full.
    pub fn record(&mut self, entry: Entry) {
        if self.entries.len() >= JOURNAL_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The `limit` most recent entries, oldest first.
    pub fn tail(&self, limit: usize) -> Vec<&Entry> {
        let skip = self.entries.len().saturating_sub(limit);
        self.entries.iter().skip(skip).collect()
    }

    /// Entries with `seq > after`, oldest first — what a follower asks for.
    pub fn after(&self, after: u64) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.seq > after).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The highest sequence number handed out so far (0 when nothing happened yet).
    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, data: &str) -> Entry {
        Entry {
            seq,
            at: 1_700_000_000.0,
            src: "test".into(),
            dst: "edit".into(),
            data: data.into(),
            attr: "addr=:42".into(),
            kind: "text".into(),
            rule: Some(7),
            verdict: "OK edit".into(),
        }
    }

    #[test]
    fn the_ring_drops_the_oldest_and_keeps_counting() {
        let mut journal = Journal::new();
        for i in 0..(JOURNAL_CAPACITY + 10) {
            let seq = journal.next_seq();
            journal.record(entry(seq, &format!("message {i}")));
        }
        assert_eq!(journal.len(), JOURNAL_CAPACITY);
        let tail = journal.tail(1);
        assert_eq!(tail[0].data, format!("message {}", JOURNAL_CAPACITY + 9));
        assert_eq!(journal.last_seq() as usize, JOURNAL_CAPACITY + 9);
    }

    #[test]
    fn after_returns_only_newer_entries() {
        let mut journal = Journal::new();
        for i in 0..5 {
            let seq = journal.next_seq();
            journal.record(entry(seq, &format!("m{i}")));
        }
        let newer = journal.after(2);
        assert_eq!(newer.len(), 2);
        assert_eq!(newer[0].seq, 3);
        assert_eq!(newer[1].seq, 4);
        assert!(journal.after(99).is_empty());
    }

    #[test]
    fn tail_is_oldest_first_and_clamped() {
        let mut journal = Journal::new();
        for i in 0..3 {
            let seq = journal.next_seq();
            journal.record(entry(seq, &format!("m{i}")));
        }
        assert_eq!(journal.tail(10).len(), 3);
        assert_eq!(journal.tail(0).len(), 0);
        assert_eq!(journal.tail(1)[0].data, "m2");
    }

    #[test]
    fn json_escapes_the_awkward_characters() {
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb\tc"), "\"a\\nb\\tc\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
        assert_eq!(json_string("откуда!"), "\"откуда!\"");
    }

    #[test]
    fn an_entry_serialises_flat_and_parsable() {
        let json = entry(3, "src/main.rs:42").to_json();
        assert!(json.starts_with("{\"seq\":3,"), "{json}");
        assert!(json.contains("\"rule\":7"), "{json}");
        assert!(json.contains("\"data\":\"src/main.rs:42\""), "{json}");
        assert!(
            !json.contains('\n'),
            "a journal line must not contain a newline"
        );
        assert_eq!(
            json.matches('"').count() % 2,
            0,
            "quotes must pair up: {json}"
        );
    }

    #[test]
    fn a_missing_rule_becomes_null_not_a_string() {
        let mut e = entry(1, "x");
        e.rule = None;
        assert!(e.to_json().contains("\"rule\":null"));
    }

    #[test]
    fn truncation_counts_characters() {
        // 10 Cyrillic characters are 20 bytes: a byte-based cut would split a character.
        let text = "очень длинный текст";
        let cut = Entry::truncate(text, 5);
        assert_eq!(cut.chars().count(), 6, "five characters plus the ellipsis");
        assert!(cut.ends_with('…'));
        assert_eq!(Entry::truncate("short", 10), "short");
    }
}
