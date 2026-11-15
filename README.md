# plumber

**Plan 9's plumber, rebuilt: a program says what a piece of text *is*, and the rules decide which
program opens it.**

Every desktop today is a pile of integrations. A log viewer can open an editor at a line only if
someone wrote that twice; a build log can open a stack frame only in the IDE that produced it; a
parts catalogue cannot hand a part number to anything at all. In 1992 Plan 9 solved this at the
system level, with one daemon and a text file of rules, and then the idea was lost with the system.

This is that idea again, on Windows/Linux/macOS: one daemon, one rules file, and messages that say
what the data *is* rather than where it should go.

```
log line "src/main.rs:42" ──▶ plumberd ──rules──▶ port "edit" ──▶ your editor at line 42
"https://9fans.net"        ──▶         ──rules──▶ port "web"  ──▶ browser
"EPLAN-XB-24"              ──▶         ──rules──▶ port "catalog" ──▶ catalogue app
```

## Primary sources

The behaviour is not invented here; it is read from the original documentation:

- [`plumber(4)`](https://man.9front.org/4/plumber) — the daemon: `send`, `rules`, one output port
  per destination, a copy to every reader, `plumb client`/`plumb start` when nobody is reading.
- [`plumb(6)`](https://man.9front.org/6/plumb) — the message format (seven fields, `data` as the
  only field allowed to contain newlines) and the rules grammar (`type is`, `data matches`,
  `arg isfile`, `data set`, `attr add`, `plumb to`, `plumb start`, first rule set wins).

## What is faithful, and what is different

| | Plan 9 | here |
|---|---|---|
| message format | seven fields, byte count before data | **identical** (`src/lib.rs`, `message.rs`) |
| rules | rule sets separated by blank lines, macros, `include` | **identical grammar** (`rules.rs`) |
| delivery | a copy to every reader of the port; sender gets an error when nobody is there | **same**, over loopback TCP |
| transport | a mounted file server at `/mnt/plumb` | **loopback TCP**, because Windows has no mountable per-process file server |
| rules reload | `cat plumbing > /mnt/plumb/rules` | **on file modification time**, so a text editor is enough |
| `plumb start` | runs a command | **opt-in** (`--allow-start`): a rules file is executable content |

## Quick start

```sh
cargo build --release

# 1. the daemon (writes its port to %LOCALAPPDATA%\plumber\port on Windows, ~/.plumber/port elsewhere)
./target/release/plumberd --rules plumbing.example

# 2. a reader for the `edit` port — this is what an editor would do
./target/release/plumb read edit

# 3. anything that produces text can now hand over an address
./target/release/plumb send "src/main.rs:42"
#   OK edit
#   the reader prints: 42	src/main.rs

# ask what the rules would do with something, without any daemon:
./target/release/plumb test --rules plumbing.example "https://9fans.net"
```

## The rules file

A rule file is a sequence of **rule sets**, separated by blank lines. The first set whose patterns
all match fires; the rest are ignored for that message. Patterns inspect the message, actions
dispose of it.

```plumb
addr=':([0-9]+)'                       # macros are expanded when the file is read

type is text                           # pattern: object, verb, argument
data matches '([a-zA-Z0-9_/\-]+)('$addr')?'
arg isfile $1                          # does the match name a file that exists?
data set $file                         # rewrite the message to the resolved path
attr add addr=$2                       # carry the line number as an attribute
plumb to edit                          # action: the destination port
```

Objects: `src`, `dst`, `wdir`, `type`, `attr`, `data`, `arg`. Verbs: `is`, `isfile`, `isdir`,
`matches`, `set`, `add`, `delete`. Actions: `plumb to`, `plumb start`, `plumb client`,
`plumb hide`. Substitutions: `$0`…`$n` (captures), `$file`, `$dir`, `$wdir`, `$src`, `$dst`,
`$type`.

## Opening, not just delivering

A reader that actually opens what arrives is `plumber-open`:

```sh
plumber-open edit                                     # built-in chain, first program found wins
plumber-open web    --cmd 'firefox {data}'
plumber-open pdf    --once --dry-run                  # show the command line, open nothing
```

Placeholders: `{data}`, `{file}`, `{line}`, `{page}`, `{addr}`, `{dir}`, `{attr:name}`. A port has a
default opener — the table Plan 9 kept in `/lib/plumbing` next to the rules — and `--cmd` replaces
it. The built-in list is a fallback chain: a candidate whose program is not on `PATH` is skipped and
the next one is tried, and if none is installed the opener says which ones it tried instead of
failing silently.

| port | default opener (first program found in `PATH` wins) |
|---|---|
| `edit` | `code -g {file}:{line}` → `zed {file}:{line}` → `gvim`/`vim`/`nvim +{line} {file}` → `subl` → `kate --line` → `micro` → `notepad` |
| `web` | `xdg-open {data}` → `open {data}` (macOS) → `cmd /c start "" {data}` (Windows) |
| `pdf` | `zathura -P {page} {file}` → `pdfcraft {file} --page {page}` → `okular --page` → `evince` → `xdg-open` → `open` → `cmd /c start` |
| `scene` | `cadcraft {file}` |

The chain is ordered by how much of the message a program can use — a viewer that jumps to a line or
a page comes before one that cannot — and the portable fallbacks come last. One list, every desktop.

With the PDF rule from `plumbing.example`, `manual.pdf:7` in any log becomes
`pdfcraft <resolved path> --page 7` — the page number travels as an attribute, so the viewer does
not have to parse the path again:

```plumb
type is text
data matches '([a-zA-Z0-9_/\-]+\.pdf)('$addr')?'
arg isfile $1
data set $file
attr add page=$2
plumb to pdf
```

## Why this is useful now

The integration problem is back, in a worse form: agents and desktop tools each hard-wire the tools
they know. A rule engine between them means a program does not need to know *who* will open its
output — only what the output is. That is the whole trick, and it is forty years old.

## Tests

```sh
cargo test
```

Forty tests, no mocks, and the CI runs them on Linux, Windows and macOS.

Of the rules and the daemon (13): the wire format round-trips (including data with newlines), the
first rule set wins, an unmatched message is refused to the sender, an existing path is rewritten to
its resolved form, a missing file falls through to the next rule set, macros expand — including
inside a quoted regular expression, a macro named like a rule variable does not hijack `$file`, a
bad regex names its line, a rule set without a pattern is refused, a message for a port nobody reads
is held and delivered to the reader that arrives later, and two end-to-end tests drive a real daemon
over a real socket.

Of the opener (12): a line number fills the editor command, a missing line number drops the colon
instead of the file, a missing line drops vim's `+` too, a page message becomes
`pdfcraft … --page 7`, `{attr:name}` works, a quoted empty argument survives the split (Windows
`start` needs it), the built-in chain stays page-aware and portable, the fallback chain skips
programs that are not installed, and a port with no opener says so.

Of the message format (7, new): a wire round-trip keeps every field, `ndata` counts bytes rather than
characters, a cut that lands inside a multi-byte character does not panic, trailing bytes beyond
`ndata` are ignored, a short message is an error, attributes with spaces survive, and `Display`
truncates by characters.

Of the daemon binary (8, new): it answers `PING`, serves the rules it is running, lists the ports from
the rules, routes a message to a real reader with its attributes, explains an unmatched message and
an unread port, reloads a changed rules file, answers malformed frames instead of hanging up, refuses
an absurd frame size, answers a frame the client cut short (half-closed socket), and rejects an
unknown command — each test starting the actual `plumberd`.

## Website

[`kostyk348.github.io/plumber`](https://kostyk348.github.io/plumber/) — the manual: install, the rules
grammar with a verified cookbook, the line protocol with shell and Python clients, and how to wire it
into a desktop (user service, MIME handlers, editor hooks).

## Roadmap

- A Windows named-pipe transport next to the loopback socket, so an app can read a port without a
  TCP listener, and a real 9P file server for the platforms that can mount one.
- Rules for `doi:10.5281/zenodo…` links (to the museum's own bibliography) and for git object ids
  (to `git show`).
- A `.desktop` handler and a `systemd --user` unit shipped with the repo, so "install and it works"
  needs no manual steps. (Both are documented on the site; generating them is the next step.)
- A D-Bus or 9P transport for sandboxed apps, where loopback TCP is not reachable.

Done since the first version: `plumber-open` (a reader that opens), the `page.pdf:12` rule that hands
a page number to PdfCraft, the hold queue for a port whose reader starts later, the portable opener
chains (`vim`, `zed`, `zathura`, `okular`, `open`), hardened framing (byte-accurate `ndata` without
panics, a 16 MiB frame limit, a 5 s read timeout), the multi-page manual, and CI on three platforms.

## Licence

MIT OR Apache-2.0.
