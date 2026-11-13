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

## Why this is useful now

The integration problem is back, in a worse form: agents and desktop tools each hard-wire the tools
they know. A rule engine between them means a program does not need to know *who* will open its
output — only what the output is. That is the whole trick, and it is forty years old.

## Tests

```sh
cargo test
```

Ten tests, no mocks: the wire format round-trips (including data with newlines), the first rule set
wins, an unmatched message is discarded, a path that exists is rewritten to its resolved form, a
missing file falls through to the next rule set, macros expand, a bad regex names its line, a rule
set without a pattern is refused, and one end-to-end test drives a real daemon over a real socket
(reader subscribes, sender sends, reader receives the rewritten message).

## Roadmap

- `plumb client` holding a message until the port opens (implemented as a hold queue; needs a test
  that a reader arriving later receives it).
- A Windows named-pipe transport, and a real 9P file server for the platforms that can mount one.
- Starter rules for: logs + PdfCraft (`page.pdf:12`), EPLAN catalogue part numbers, `doi:` links,
  git object ids.

## Licence

MIT OR Apache-2.0.
