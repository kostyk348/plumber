//! The plumb message and its textual form.
//!
//! `plumb(6)` fixes the format: seven fields, in order — `src`, `dst`, `wdir`, `type`, `attr`,
//! `ndata`, `data` — one per line, with `ndata` an accurate byte count of `data`, and the empty
//! line standing for a missing field. Only `data` may contain newlines, which is exactly why it
//! is length-prefixed instead of escaped.
//!
//! Keeping the wire format identical to Plan 9's is deliberate: a message from a 1995 program and
//! a message from this one are the same bytes, so anything that already speaks the format works.

use std::fmt;

/// Order of the fields on the wire, as `plumb(6)` specifies it.
pub const FIELDS: [&str; 7] = ["src", "dst", "wdir", "type", "attr", "ndata", "data"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Message {
    /// Application or service that produced the message.
    pub src: String,
    /// Destination port; normally assigned by the rules, not by the sender.
    pub dst: String,
    /// Working directory, used when the data is a file name.
    pub wdir: String,
    /// Form of the data; only `text` is meaningful today.
    pub kind: String,
    /// `name=value` pairs, separated by spaces. Values with spaces are quoted.
    pub attr: Vec<(String, String)>,
    /// The data itself. The only field allowed to contain newlines.
    pub data: String,
}

impl Message {
    pub fn new(src: &str, kind: &str, data: &str) -> Self {
        Self { src: src.to_string(), dst: String::new(), wdir: String::new(), kind: kind.to_string(), attr: Vec::new(), data: data.to_string() }
    }

    /// The wire form, exactly as `plumb(6)` describes it.
    pub fn to_wire(&self) -> String {
        let attr = self
            .attr
            .iter()
            .map(|(k, v)| if v.contains([' ', '"']) { format!("{k}=\"{v}\"") } else { format!("{k}={v}") })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n{}",
            self.src,
            self.dst,
            self.wdir,
            self.kind,
            attr,
            self.data.len(),
            self.data
        )
    }

    /// Parse the wire form. A trailing newline after `data` is allowed and ignored, because a
    /// program that writes messages to a stream usually ends them with one.
    pub fn from_wire(text: &str) -> Result<Self, String> {
        let mut rest = text;
        let mut fields: Vec<String> = Vec::with_capacity(7);
        for (index, name) in FIELDS.iter().enumerate() {
            if index == FIELDS.len() - 1 {
                // `data` takes everything up to the declared byte count.
                break;
            }
            match rest.find('\n') {
                Some(end) => {
                    fields.push(rest[..end].to_string());
                    rest = &rest[end + 1..];
                }
                None => return Err(format!("message ends before the {name} field")),
            }
        }
        let ndata: usize = fields[5].trim().parse().map_err(|_| format!("ndata is not a number: {:?}", fields[5]))?;
        if rest.len() < ndata {
            return Err(format!("data is {ndata} bytes but only {} arrived", rest.len()));
        }
        let data = rest[..ndata].to_string();
        Ok(Self {
            src: fields[0].clone(),
            dst: fields[1].clone(),
            wdir: fields[2].clone(),
            kind: fields[3].clone(),
            attr: parse_attr(&fields[4]),
            data,
        })
    }

    /// One attribute by name.
    pub fn attr_get(&self, name: &str) -> Option<&str> {
        self.attr.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn attr_set(&mut self, name: &str, value: &str) {
        match self.attr.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = value.to_string(),
            None => self.attr.push((name.to_string(), value.to_string())),
        }
    }

    pub fn attr_delete(&mut self, name: &str) {
        self.attr.retain(|(k, _)| k != name);
    }
}

/// `name=value` pairs, with a quoted value allowed to contain spaces and `=`.
fn parse_attr(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != '=' && !bytes[i].is_whitespace() {
            i += 1;
        }
        let name: String = bytes[start..i].iter().collect();
        if i >= bytes.len() || bytes[i] != '=' {
            if !name.is_empty() {
                out.push((name, String::new()));
            }
            continue;
        }
        i += 1; // '='
        let value = if i < bytes.len() && bytes[i] == '"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != '"' {
                i += 1;
            }
            let value: String = bytes[start..i].iter().collect();
            i += 1;
            value
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_whitespace() {
                i += 1;
            }
            bytes[start..i].iter().collect()
        };
        out.push((name, value));
    }
    out
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} → {}: {:?}", if self.src.is_empty() { "?" } else { &self.src }, if self.dst.is_empty() { "(no port)" } else { &self.dst }, truncate(&self.data, 60))
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}
