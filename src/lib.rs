//! Plan 9's plumber, rebuilt for machines that have no `/mnt`.
//!
//! The original is a user-level file server: programs write a message to `/mnt/plumb/send`, the
//! rules decide which *port* it belongs to, and every program that has that port open reads a copy.
//! The interesting part was never the file system — it was the rule that **the message says what
//! the data is, and the rules decide who opens it**. Today that is exactly what is missing between
//! desktop programs: a log viewer cannot hand a `file:line` to an editor, a parts catalogue cannot
//! hand a part number to a browser, a build log cannot hand a stack frame to a debugger, unless
//! somebody wrote that integration twice.
//!
//! What is the same as Plan 9 here:
//!
//! - the message format ([`message::Message`], the seven fields of `plumb(6)`), byte for byte;
//! - the rules grammar ([`rules::Rules`]): rule sets, `type is`, `data matches`, `arg isfile`,
//!   `data set`, `attr add`, `plumb to`, `plumb start`, first match wins;
//! - the delivery model: a copy to every reader of the destination port, and a failure to the
//!   sender when nobody is listening.
//!
//! What is different, and why:
//!
//! - **Loopback TCP instead of a mounted file system.** Windows has no mountable per-process file
//!   server, so `send` and the ports are a line protocol on `127.0.0.1` (see [`daemon`]). The
//!   client still does not choose a port: it sends a message and is told where it went.
//! - **Rules are reloaded on their modification time**, because sending a signal to a Windows
//!   process is not a thing you want to require from a text editor.

pub mod daemon;
pub mod message;
pub mod rules;

pub use daemon::{Config, Daemon};
pub use message::Message;
pub use rules::{Action, Route, Rules};
