//! The plumbing rules: patterns that inspect a message, actions that dispose of it.
//!
//! The grammar is `plumb(6)`: a rule file is a sequence of *rule sets* separated by blank lines.
//! A rule set is a list of one-line patterns and actions; the first rule set whose patterns all
//! match fires, and the rest are ignored for that message. Macros (`addr=':([0-9]+)'`) are
//! expanded while the file is read, `include` splices another file in, and `#` starts a comment.
//!
//! What this implementation deliberately keeps:
//!
//! - **The rule decides the destination.** The sender does not name a port; it says what the data
//!   *is*, and the rules map that to `plumb to`.
//! - **First match wins.** Order in the file is the priority, so a file with a line number is
//!   handled before a bare URL.
//! - **`arg` tests the substitution, not the message.** `arg isfile $0` is how a rule asks "does
//!   this look like a path that exists?", and that is what makes $file and $dir available below.
//! - **Rewrites are permanent.** `data set $file` changes the message that the receiver gets.

use std::collections::BTreeSet;
use std::path::Path;

use regex::Regex;

use crate::message::Message;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obj {
    Src,
    Dst,
    Wdir,
    Kind,
    Attr,
    Data,
    /// The argument of the current rule, after substitution — the object `isfile`/`isdir` test.
    Arg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Is,
    IsFile,
    IsDir,
    Matches,
    Add,
    Delete,
    Set,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub obj: Obj,
    pub verb: Verb,
    pub arg: String,
    /// Compiled at parse time for `matches`, after macro expansion.
    pub regex: Option<Regex>,
    /// Line in the rules file, for error messages and for `plumb list`.
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `plumb to <port>`
    To(String),
    /// `plumb start <command>`: run it, discard the message.
    Start(String),
    /// `plumb client <command>`: run it and hold the message until the port is opened.
    Client(String),
    /// `plumb hide`: deliver nowhere.
    Hide,
}

#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    pub patterns: Vec<Pattern>,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Default)]
pub struct Rules {
    pub sets: Vec<RuleSet>,
    /// Every port the file names in a `plumb to`, so the daemon can create them up front.
    pub ports: BTreeSet<String>,
}

/// What the rules decided for one message.
#[derive(Clone, Debug, Default)]
pub struct Route {
    pub dst: Option<String>,
    pub start: Option<String>,
    pub client: Option<String>,
    pub message: Message,
    /// The rule set that fired, by its first line, or `None` when nothing matched.
    pub rule_line: Option<usize>,
}

impl Route {
    pub fn matched(&self) -> bool {
        self.rule_line.is_some()
    }
}

/// The variables a substitution can use.
#[derive(Clone, Debug, Default)]
struct Vars {
    caps: Vec<String>,
    file: String,
    dir: String,
    wdir: String,
    src: String,
    dst: String,
    kind: String,
}

impl Vars {
    fn from_message(msg: &Message) -> Self {
        Self {
            caps: Vec::new(),
            file: String::new(),
            dir: String::new(),
            wdir: msg.wdir.clone(),
            src: msg.src.clone(),
            dst: msg.dst.clone(),
            kind: msg.kind.clone(),
        }
    }
}

/// Expand `$0`, `$1`, `$file`, `$dir`, `$wdir`, `$src`, `$dst`, `$type` and `$$`.
fn expand(text: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= chars.len() {
            out.push('$');
            break;
        }
        if chars[i] == '$' {
            out.push('$');
            i += 1;
            continue;
        }
        // A capture number, or a named variable.
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let index: usize = chars[start..i].iter().collect::<String>().parse().unwrap_or(usize::MAX);
            if let Some(value) = vars.caps.get(index) {
                out.push_str(value);
            }
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
            i += 1;
        }
        let name: String = chars[start..i].iter().collect();
        match name.as_str() {
            "file" => out.push_str(&vars.file),
            "dir" => out.push_str(&vars.dir),
            "wdir" => out.push_str(&vars.wdir),
            "src" => out.push_str(&vars.src),
            "dst" => out.push_str(&vars.dst),
            "type" => out.push_str(&vars.kind),
            _ => {
                // An unknown name is left alone rather than silently dropped: a typo in a rule
                // should be visible in the message the receiver gets.
                out.push('$');
                out.push_str(&name);
            }
        }
    }
    out
}

fn obj_of(word: &str) -> Option<Obj> {
    Some(match word {
        "src" => Obj::Src,
        "dst" => Obj::Dst,
        "wdir" => Obj::Wdir,
        "type" => Obj::Kind,
        "attr" => Obj::Attr,
        "data" => Obj::Data,
        "arg" => Obj::Arg,
        _ => return None,
    })
}

fn verb_of(word: &str) -> Option<Verb> {
    Some(match word {
        "is" => Verb::Is,
        "isfile" => Verb::IsFile,
        "isdir" => Verb::IsDir,
        "matches" => Verb::Matches,
        "add" => Verb::Add,
        "delete" => Verb::Delete,
        "set" => Verb::Set,
        _ => return None,
    })
}

/// Split a rules line into words, keeping a quoted argument together and unquoting it.
///
/// A `'` toggles quoting wherever it appears, because plumb rules nest them: the argument of
/// `data matches` is one quoted string that contains a quoted macro (`('$addr')?`). Treating the
/// inner pair as literal characters makes the regular expression match nothing at all, which is a
/// silent failure — the rule set simply never fires.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut has_word = false;
    for c in line.chars() {
        match c {
            '\'' => {
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

impl Rules {
    /// Read a rules file. `base` is the directory relative `include`s are resolved against.
    pub fn parse(text: &str, base: &Path) -> Result<Self, String> {
        let mut rules = Rules::default();
        rules.parse_into(text, base, 0)?;
        Ok(rules)
    }

    fn parse_into(&mut self, text: &str, base: &Path, depth: usize) -> Result<(), String> {
        if depth > 8 {
            return Err("include nesting is deeper than 8 files (a cycle?)".into());
        }
        let mut macros: Vec<(String, String)> = Vec::new();
        let mut current = RuleSet::default();
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                // A blank line ends a rule set.
                if !current.patterns.is_empty() || !current.actions.is_empty() {
                    for action in &current.actions {
                        if let Action::To(port) = action {
                            self.ports.insert(port.clone());
                        }
                    }
                    self.sets.push(std::mem::take(&mut current));
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("include ") {
                let name = rest.trim();
                let path = if Path::new(name).is_absolute() { std::path::PathBuf::from(name) } else { base.join(name) };
                let text = std::fs::read_to_string(&path).map_err(|e| format!("include {name}: {e}"))?;
                // An include brings its own rule sets; it is spliced where the line stands, so the
                // current set is flushed first.
                if !current.patterns.is_empty() || !current.actions.is_empty() {
                    self.sets.push(std::mem::take(&mut current));
                }
                self.parse_into(&text, path.parent().unwrap_or(base), depth + 1)?;
                continue;
            }
            // A macro: `name='value'` or `name = 'value'`.
            if let Some(eq) = line.find('=') {
                let name = line[..eq].trim();
                let value = line[eq + 1..].trim().trim_matches('\'');
                let looks_like_macro = !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                    && !name.starts_with("data")
                    && !name.starts_with("arg");
                if looks_like_macro {
                    macros.push((name.to_string(), value.to_string()));
                    continue;
                }
            }
            let expanded = expand_macros(line, &macros);
            let parts = words(&expanded);
            if parts.is_empty() {
                continue;
            }
            if parts[0] == "plumb" {
                if parts.len() < 2 {
                    return Err(format!("line {line_no}: `plumb` needs an action"));
                }
                let action = match parts[1].as_str() {
                    "to" => Action::To(parts.get(2).cloned().ok_or_else(|| format!("line {line_no}: `plumb to` needs a port"))?),
                    "start" => Action::Start(parts[2..].join(" ")),
                    "client" => Action::Client(parts[2..].join(" ")),
                    "hide" => Action::Hide,
                    other => return Err(format!("line {line_no}: unknown action `plumb {other}`")),
                };
                if current.patterns.is_empty() {
                    return Err(format!("line {line_no}: a rule set needs at least one pattern before an action"));
                }
                current.actions.push(action);
                continue;
            }
            if parts.len() < 2 {
                return Err(format!("line {line_no}: expected `<object> <verb> <argument>`"));
            }
            let obj = obj_of(&parts[0]).ok_or_else(|| format!("line {line_no}: unknown object `{}`", parts[0]))?;
            let verb = verb_of(&parts[1]).ok_or_else(|| format!("line {line_no}: unknown verb `{}`", parts[1]))?;
            let arg = parts[2..].join(" ");
            if (verb == Verb::Matches) && arg.is_empty() {
                return Err(format!("line {line_no}: `matches` needs a regular expression"));
            }
            let regex = if verb == Verb::Matches {
                Some(Regex::new(&arg).map_err(|e| format!("line {line_no}: bad regular expression: {e}"))?)
            } else {
                None
            };
            current.patterns.push(Pattern { obj, verb, arg, regex, line: line_no });
        }
        if !current.patterns.is_empty() || !current.actions.is_empty() {
            for action in &current.actions {
                if let Action::To(port) = action {
                    self.ports.insert(port.clone());
                }
            }
            self.sets.push(current);
        }
        if self.sets.is_empty() {
            return Err("the rules file has no rule sets".into());
        }
        Ok(())
    }

    /// Route one message: the first firing rule set wins.
    pub fn route(&self, msg: &Message) -> Route {
        for set in &self.sets {
            let mut working = msg.clone();
            let mut vars = Vars::from_message(&working);
            let mut ok = true;
            for pattern in &set.patterns {
                if !apply(pattern, &mut working, &mut vars) {
                    ok = false;
                    break;
                }
                vars.src = working.src.clone();
                vars.dst = working.dst.clone();
                vars.kind = working.kind.clone();
                vars.wdir = working.wdir.clone();
            }
            if !ok {
                continue;
            }
            let mut route = Route { message: working, rule_line: set.patterns.first().map(|p| p.line), ..Default::default() };
            for action in &set.actions {
                match action {
                    Action::To(port) => route.dst = Some(expand(port, &vars)),
                    Action::Start(command) => route.start = Some(expand(command, &vars)),
                    Action::Client(command) => route.client = Some(expand(command, &vars)),
                    Action::Hide => {}
                }
            }
            if let Some(dst) = &route.dst {
                route.message.dst = dst.clone();
            }
            return route;
        }
        Route { message: msg.clone(), ..Default::default() }
    }
}

/// Expand `$macro` names that were defined in the file. Captures (`$0`) are left for later.
///
/// A macro whose name is also a rule variable (`file`, `dir`, `wdir`, `src`, `dst`, `type`) is
/// **not** expanded: `$file` after an `isfile` pattern means "the resolved path", and plumb(6)'s
/// own example defines a macro called `file` for its regular expressions. Expanding that name
/// would replace `data set $file` with the text of a regular expression — which is a confusing
/// failure to debug, so the reserved names win.
fn expand_macros(line: &str, macros: &[(String, String)]) -> String {
    const RESERVED: [&str; 6] = ["file", "dir", "wdir", "src", "dst", "type"];
    let mut out = line.to_string();
    for (name, value) in macros {
        if RESERVED.contains(&name.as_str()) {
            continue;
        }
        out = out.replace(&format!("${name}"), value);
    }
    out
}

/// Apply one pattern. Returns false when a predicate fails.
fn apply(pattern: &Pattern, msg: &mut Message, vars: &mut Vars) -> bool {
    let arg = expand(&pattern.arg, vars);
    let subject = match pattern.obj {
        Obj::Src => msg.src.clone(),
        Obj::Dst => msg.dst.clone(),
        Obj::Wdir => msg.wdir.clone(),
        Obj::Kind => msg.kind.clone(),
        Obj::Attr => msg.attr.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "),
        Obj::Data => msg.data.clone(),
        Obj::Arg => arg.clone(),
    };
    match pattern.verb {
        Verb::Is => subject == arg,
        Verb::IsFile => {
            let path = resolve(&subject, &msg.wdir);
            let exists = path.is_file();
            if exists {
                vars.file = path.to_string_lossy().into_owned();
            }
            exists
        }
        Verb::IsDir => {
            let path = resolve(&subject, &msg.wdir);
            let exists = path.is_dir();
            if exists {
                vars.dir = path.to_string_lossy().into_owned();
            }
            exists
        }
        Verb::Matches => {
            let Some(regex) = &pattern.regex else { return false };
            match regex.captures(&subject) {
                Some(caps) => {
                    vars.caps = (0..caps.len()).map(|i| caps.get(i).map(|m| m.as_str().to_string()).unwrap_or_default()).collect();
                    true
                }
                None => false,
            }
        }
        Verb::Set => {
            match pattern.obj {
                Obj::Data => msg.data = arg,
                Obj::Src => msg.src = arg,
                Obj::Dst => msg.dst = arg,
                Obj::Wdir => msg.wdir = arg,
                Obj::Kind => msg.kind = arg,
                // `attr set` is not in plumb(6); `attr add` and `attr delete` are.
                Obj::Attr | Obj::Arg => return false,
            }
            true
        }
        Verb::Add => {
            if pattern.obj != Obj::Attr {
                return false;
            }
            for pair in split_pairs(&arg) {
                msg.attr_set(&pair.0, &pair.1);
            }
            true
        }
        Verb::Delete => {
            if pattern.obj != Obj::Attr {
                return false;
            }
            msg.attr_delete(&arg);
            true
        }
    }
}

/// A relative path in a rule is relative to the message's working directory, as in Plan 9.
fn resolve(path: &str, wdir: &str) -> std::path::PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() || wdir.is_empty() {
        candidate.to_path_buf()
    } else {
        Path::new(wdir).join(candidate)
    }
}

fn split_pairs(text: &str) -> Vec<(String, String)> {
    text.split_whitespace()
        .filter_map(|pair| pair.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}
