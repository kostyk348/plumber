//! `plumber-console` — a window on the plumbing: what the daemon is doing, and a place to try a
//! rule before it bites.
//!
//! Built with egui: one binary, no web view, no JavaScript bridge. The console talks to the daemon
//! over the same line protocol as `plumb` and, for the rule tester, uses the very same [`Rules`]
//! parser that the daemon runs — so what the tester shows is what would happen.
//!
//! ```text
//! cargo run --release --features gui --bin plumber-console
//! ```

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use plumber::message::Message;
use plumber::rules::Rules;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([980.0, 720.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("plumber — консоль"),
        ..Default::default()
    };
    eframe::run_native(
        "plumber — консоль",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            Ok(Box::new(Console::new()))
        }),
    )
}

/// egui's bundled Ubuntu font covers Cyrillic, but the *fallback* chain does not always pick it for
/// text that starts with Latin. Putting the bundled font first for both families removes the guess.
fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            // Keep the fallbacks (emoji, CJK) but make Ubuntu the primary face.
            if let Some(pos) = list.iter().position(|name| name == "Ubuntu-Light") {
                let ubuntu = list.remove(pos);
                list.insert(0, ubuntu);
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// One journal entry, as parsed from the daemon's JSON line.
#[derive(Clone, Default)]
struct Event {
    seq: u64,
    at: f64,
    src: String,
    dst: String,
    attr: String,
    rule: Option<u64>,
    verdict: String,
    data: String,
}

impl Event {
    fn parse(line: &str) -> Option<Self> {
        if !line.starts_with('{') {
            return None;
        }
        Some(Self {
            seq: num(line, "seq")?,
            at: num_f(line, "at").unwrap_or(0.0),
            src: string(line, "src").unwrap_or_default(),
            dst: string(line, "dst").unwrap_or_default(),
            attr: string(line, "attr").unwrap_or_default(),
            rule: num(line, "rule"),
            verdict: string(line, "verdict").unwrap_or_default(),
            data: string(line, "data").unwrap_or_default(),
        })
    }

    fn clock(&self) -> String {
        let total = self.at as u64;
        let h = (total / 3600) % 24;
        let m = (total / 60) % 60;
        let s = total % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    fn ok(&self) -> bool {
        self.verdict.starts_with("OK")
    }
}

fn value_start<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    Some(&line[start..])
}

fn num(line: &str, key: &str) -> Option<u64> {
    let rest = value_start(line, key)?;
    if rest.starts_with("null") {
        return None;
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn num_f(line: &str, key: &str) -> Option<f64> {
    let rest = value_start(line, key)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn string(line: &str, key: &str) -> Option<String> {
    let rest = value_start(line, key)?;
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(c) = char::from_u32(code) {
                            out.push(c);
                        }
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Where the daemon listens: the same file `plumb` reads.
fn port_file() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("plumber").join("port");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".plumber").join("port")
}

fn rules_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PLUMBER_RULES") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("plumber").join("plumbing")
}

fn daemon_port() -> Option<u16> {
    std::fs::read_to_string(port_file())
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// One request in, all lines out. The daemon closes after answering (RULES, PORTS, EVENTS).
fn request(command: &str, timeout: Duration) -> Result<Vec<String>, String> {
    let port = daemon_port().ok_or_else(|| "демон не запущен: нет файла порта".to_string())?;
    let stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("нет связи с 127.0.0.1:{port}: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    writeln!(writer, "{command}").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(BufReader::new(stream)
        .lines()
        .map_while(Result::ok)
        .collect())
}

/// What the rule tester decided.
struct TestOutcome {
    input: String,
    lines: Vec<String>,
}

struct Console {
    events: Vec<Event>,
    last_seq: Option<u64>,
    ports: Vec<(String, usize)>,
    status: String,
    connected: bool,
    rules_text: String,
    rules_dirty: bool,
    rules_note: String,
    send_text: String,
    send_note: String,
    tester_text: String,
    tester: Option<TestOutcome>,
    autoscroll: bool,
    last_poll: Instant,
}

impl Console {
    fn new() -> Self {
        let mut console = Self {
            events: Vec::new(),
            last_seq: None,
            ports: Vec::new(),
            status: "проверяю демон…".to_string(),
            connected: false,
            rules_text: String::new(),
            rules_dirty: false,
            rules_note: String::new(),
            send_text: String::new(),
            send_note: String::new(),
            tester_text: String::new(),
            tester: None,
            autoscroll: true,
            last_poll: Instant::now() - Duration::from_secs(10),
        };
        console.load_rules();
        console.poll();
        console
    }

    fn load_rules(&mut self) {
        match std::fs::read_to_string(rules_path()) {
            Ok(text) => {
                self.rules_text = text;
                self.rules_dirty = false;
                self.rules_note = format!("загружено из {}", rules_path().display());
            }
            Err(e) => {
                self.rules_note = format!("не читается {}: {e}", rules_path().display());
            }
        }
    }

    fn save_rules(&mut self) {
        match std::fs::write(rules_path(), &self.rules_text) {
            Ok(()) => {
                self.rules_dirty = false;
                self.rules_note = "сохранено; демон перечитает по времени изменения".to_string();
            }
            Err(e) => self.rules_note = format!("не записалось: {e}"),
        }
    }

    fn poll(&mut self) {
        self.last_poll = Instant::now();
        match request("PORTS", Duration::from_secs(2)) {
            Ok(lines) => {
                self.connected = true;
                self.ports = lines
                    .iter()
                    .filter_map(|line| {
                        let mut parts = line.split_whitespace();
                        let name = parts.next()?.to_string();
                        let count = parts.next()?.parse().ok()?;
                        Some((name, count))
                    })
                    .collect();
                self.status = match daemon_port() {
                    Some(port) => format!("демон работает, порт {port}"),
                    None => "демон работает".to_string(),
                };
                self.poll_events();
            }
            Err(e) => {
                self.connected = false;
                self.status = e;
            }
        }
    }

    fn poll_events(&mut self) {
        let command = match self.last_seq {
            Some(seq) => format!("EVENTS AFTER {seq}"),
            None => "EVENTS 100".to_string(),
        };
        let Ok(lines) = request(&command, Duration::from_secs(2)) else {
            return;
        };
        for line in lines {
            if let Some(event) = Event::parse(&line) {
                self.last_seq = Some(event.seq);
                self.events.push(event);
            }
        }
        if self.events.len() > 2000 {
            let cut = self.events.len() - 2000;
            self.events.drain(0..cut);
        }
    }

    fn send(&mut self) {
        let text = self.send_text.clone();
        if text.trim().is_empty() {
            return;
        }
        let port = match daemon_port() {
            Some(port) => port,
            None => {
                self.send_note = "демон не запущен".to_string();
                return;
            }
        };
        let message = Message::new("console", "text", &text);
        let wire = message.to_wire();
        let attempt = (|| -> Result<String, String> {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            write!(stream, "SEND {}\n{}", wire.len(), wire).map_err(|e| e.to_string())?;
            stream.flush().map_err(|e| e.to_string())?;
            let mut reply = String::new();
            BufReader::new(stream)
                .read_line(&mut reply)
                .map_err(|e| e.to_string())?;
            Ok(reply.trim_end().to_string())
        })();
        match attempt {
            Ok(reply) => self.send_note = reply,
            Err(e) => self.send_note = format!("не отправилось: {e}"),
        }
        self.poll();
    }

    /// Run the tester input through the same parser the daemon uses, and show the resulting message.
    fn run_tester(&mut self) {
        let base = rules_path()
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let input = self.tester_text.clone();
        let mut lines = Vec::new();
        match Rules::parse(&self.rules_text, &base) {
            Err(e) => lines.push(format!("правила не разбираются: {e}")),
            Ok(rules) => {
                let message = Message::new("tester", "text", &input);
                let route = rules.route(&message);
                match route.rule_line {
                    Some(line) => lines.push(format!("правило: строка {line}")),
                    None => lines.push("ни одно правило не совпало".to_string()),
                }
                match &route.dst {
                    Some(port) => lines.push(format!("порт: {port}")),
                    None => lines.push("порт: (нет)".to_string()),
                }
                if let Some(start) = &route.start {
                    lines.push(format!("start: {start}"));
                }
                if let Some(client) = &route.client {
                    lines.push(format!("client: {client}"));
                }
                lines.push("— сообщение, которое получит читатель —".to_string());
                lines.push(route.message.to_wire().to_string());
            }
        }
        self.tester = Some(TestOutcome { input, lines });
    }
}

impl eframe::App for Console {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll the daemon about once a second without blocking the UI thread on a socket.
        if self.last_poll.elapsed() > Duration::from_millis(900) {
            self.poll();
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (dot, color) = if self.connected {
                    ("●", egui::Color32::from_rgb(90, 200, 120))
                } else {
                    ("●", egui::Color32::from_rgb(220, 110, 110))
                };
                ui.colored_label(color, dot);
                ui.label(&self.status);
                ui.separator();
                ui.label(format!("правила: {}", rules_path().display()));
                if self.rules_dirty {
                    ui.colored_label(egui::Color32::from_rgb(230, 180, 90), "• не сохранено");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Обновить").clicked() {
                        self.poll();
                    }
                    if ui.button("Перечитать правила").clicked() {
                        self.load_rules();
                    }
                });
            });
        });

        egui::SidePanel::left("ports")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("Порты");
                if self.ports.is_empty() {
                    ui.weak("портов нет — демон не запущен?");
                }
                for (port, readers) in &self.ports {
                    let mark = if *readers > 0 { "●" } else { "○" };
                    let color = if *readers > 0 {
                        egui::Color32::from_rgb(90, 200, 120)
                    } else {
                        egui::Color32::GRAY
                    };
                    ui.horizontal(|ui| {
                        ui.colored_label(color, mark);
                        ui.monospace(port);
                        ui.weak(format!("{readers} чит."));
                    });
                }
                ui.separator();
                ui.heading("Отправить");
                ui.add(
                    egui::TextEdit::singleline(&mut self.send_text)
                        .hint_text("src/main.rs:42 или https://…")
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui.button("Отправить").clicked() {
                        self.send();
                    }
                    if ui.button("Очистить").clicked() {
                        self.send_text.clear();
                        self.send_note.clear();
                    }
                });
                if !self.send_note.is_empty() {
                    let ok = self.send_note.starts_with("OK");
                    let color = if ok {
                        egui::Color32::from_rgb(90, 200, 120)
                    } else {
                        egui::Color32::from_rgb(220, 130, 130)
                    };
                    ui.colored_label(color, &self.send_note);
                }
            });

        egui::TopBottomPanel::bottom("rules")
            .resizable(true)
            .default_height(300.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Правила");
                    if ui.button("Сохранить").clicked() {
                        self.save_rules();
                    }
                    if ui.button("Перечитать").clicked() {
                        self.load_rules();
                    }
                    if ui.button("Проверить текст →").clicked() {
                        self.run_tester();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tester_text)
                            .hint_text("текст сообщения для проверки")
                            .desired_width(260.0),
                    );
                });
                if !self.rules_note.is_empty() {
                    ui.weak(&self.rules_note);
                }
                egui::ScrollArea::vertical()
                    .id_salt("rules-scroll")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        let response = ui.add(
                            egui::TextEdit::multiline(&mut self.rules_text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(8),
                        );
                        if response.changed() {
                            self.rules_dirty = true;
                        }
                    });
                if let Some(outcome) = &self.tester {
                    ui.separator();
                    ui.monospace(format!("вход: {}", outcome.input));
                    for line in &outcome.lines {
                        ui.monospace(line);
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Журнал");
                ui.weak(format!("{} записей", self.events.len()));
                ui.checkbox(&mut self.autoscroll, "прокручивать");
                if ui.button("Очистить вид").clicked() {
                    self.events.clear();
                }
            });
            egui::ScrollArea::vertical()
                .stick_to_bottom(self.autoscroll)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for event in self.events.iter().rev().take(500) {
                        let color = if event.ok() {
                            egui::Color32::from_rgb(120, 200, 140)
                        } else {
                            egui::Color32::from_rgb(220, 130, 130)
                        };
                        ui.horizontal(|ui| {
                            ui.monospace(event.clock());
                            ui.colored_label(color, format!("{:<26}", event.verdict));
                            ui.monospace(&event.src);
                            ui.weak("->");
                            ui.monospace(&event.dst);
                            let mut text = event.data.clone();
                            if !event.attr.is_empty() {
                                text.push_str(&format!("  [{}]", event.attr));
                            }
                            if let Some(rule) = event.rule {
                                text.push_str(&format!("  строка {rule}"));
                            }
                            ui.label(text);
                        });
                    }
                });
        });
    }
}
