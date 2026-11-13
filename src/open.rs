//! Opening what a message names: the reader side of the plumber.
//!
//! A port has a default opener — the table Plan 9 kept in `/lib/plumbing` beside the rules — and a
//! template overrides it. Templates are split into words like a shell would and these placeholders
//! are filled per message:
//!
//! | placeholder | meaning |
//! |---|---|
//! | `{data}` | the message body |
//! | `{file}` | the body, or the resolved path when the message carries one |
//! | `{line}` | the line number from `addr=`, without the colon |
//! | `{page}` | the page from `page=` (a `page.pdf:12` message), else the line |
//! | `{addr}` | `addr=` as it is, colon included |
//! | `{dir}` | the working directory of the message |
//! | `{attr:name}` | any attribute by name |
//!
//! If the program a template names is not on `PATH`, the opener says so and lists what it tried,
//! instead of failing silently — which is the whole reason `--dry-run` exists.

use std::path::Path;
use std::process::Command;

use crate::message::Message;

/// The candidates for a port, most specific first. The first one whose program exists on `PATH`
/// wins; a `--cmd` template replaces the whole list.
pub fn candidates(port: &str) -> Vec<&'static str> {
    match port {
        "edit" => vec!["code -g {file}:{line}", "code {file}", "notepad {file}"],
        "web" => vec!["cmd /c start \"\" {data}", "xdg-open {data}"],
        "pdf" => vec!["pdfcraft {file} --page {page}", "cmd /c start \"\" {file}", "xdg-open {file}"],
        "scene" => vec!["cadcraft {file}"],
        _ => Vec::new(),
    }
}

/// Build and (unless asked not to) start the command for one message. Returns the command line as
/// it was run, so a caller — or a test — can see exactly what happened.
pub fn open(message: &Message, port: &str, template: Option<&str>, dry_run: bool) -> Result<String, String> {
    let vars = Vars::from(message);
    let list: Vec<String> = match template {
        Some(template) => vec![template.to_string()],
        None => candidates(port).into_iter().map(str::to_string).collect(),
    };
    if list.is_empty() {
        return Err(format!("no default opener for port {port:?}; pass --cmd 'program {{file}}'"));
    }
    let mut tried = Vec::new();
    // An explicit template is the caller's choice and is run as given — a dry run must show it
    // even when the program is not installed here. The built-in list, by contrast, is a fallback
    // chain: a candidate whose program is missing is skipped.
    let explicit = template.is_some();
    for candidate in &list {
        let words = split_words(&vars.expand(candidate));
        let Some(program) = words.first() else { continue };
        if !explicit && !on_path(program) {
            tried.push(format!("{program} (not on PATH)"));
            continue;
        }
        let line = display(&words);
        if !dry_run {
            let mut child = Command::new(program)
                .args(&words[1..])
                .spawn()
                .map_err(|e| format!("cannot start {program}: {e}"))?;
            // Do not wait: a reader that blocks on an editor stops reading its port. Reap later.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        return Ok(format!("{} {line}", if dry_run { "would run:" } else { "opened:" }));
    }
    Err(format!("nothing to open with; tried {}", tried.join(", ")))
}

/// The command line as it was run, with empty arguments quoted: `start "" {url}` loses its empty
/// title when printed plainly, and that empty argument is the difference between opening a URL and
/// opening a folder called `https:`.
pub fn display(words: &[String]) -> String {
    words
        .iter()
        .map(|word| if word.is_empty() || word.contains(' ') { format!("\"{word}\"") } else { word.clone() })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The values a template is expanded with, read from one message.
pub struct Vars {
    pub data: String,
    pub file: String,
    pub line: String,
    pub page: String,
    pub addr: String,
    pub dir: String,
    pub attrs: Vec<(String, String)>,
}

impl From<&Message> for Vars {
    fn from(message: &Message) -> Self {
        let addr = message.attr_get("addr").unwrap_or("").to_string();
        let line = addr.trim_start_matches(':').to_string();
        let page = message
            .attr_get("page")
            .map(|page| page.trim_start_matches(':').to_string())
            // A rule may put the page in `addr` instead: fall back to the line number.
            .unwrap_or_else(|| line.clone());
        Self {
            data: message.data.clone(),
            // After `data set $file` the body already *is* the resolved path.
            file: message.data.clone(),
            line,
            page,
            addr,
            dir: message.wdir.clone(),
            attrs: message.attr.clone(),
        }
    }
}

impl Vars {
    pub fn expand(&self, template: &str) -> String {
        let mut out = template.to_string();
        for (name, value) in &self.attrs {
            out = out.replace(&format!("{{attr:{name}}}"), value);
        }
        out = out
            .replace("{data}", &self.data)
            .replace("{file}", &self.file)
            .replace("{line}", &self.line)
            .replace("{page}", &self.page)
            .replace("{addr}", &self.addr)
            .replace("{dir}", &self.dir);
        // `code -g file:` with no line number opens nothing: drop the dangling colon.
        out.split(' ')
            .map(|word| word.strip_suffix(':').unwrap_or(word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Split a template into words the way a shell would, so `cmd /c start "" {data}` keeps its empty
/// argument (Windows `start` needs it before a URL).
pub fn split_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_word = false;
    for c in text.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_word = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_word {
                    out.push(current.clone());
                    current.clear();
                    has_word = false;
                }
            }
            c => {
                current.push(c);
                has_word = true;
            }
        }
    }
    if has_word {
        out.push(current);
    }
    out
}

/// Is this program runnable? A name with a separator is a path; a bare name is looked up in `PATH`,
/// with the Windows extensions so `notepad` finds `notepad.exe`.
pub fn on_path(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(dirs) = std::env::var_os("PATH") else { return false };
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD".into()).split(';').map(|ext| ext.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&dirs) {
        if dir.join(program).is_file() {
            return true;
        }
        for extension in &extensions {
            if dir.join(format!("{program}{extension}")).is_file() {
                return true;
            }
        }
    }
    false
}
