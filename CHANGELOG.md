# Changelog

All notable changes to this project. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the version numbers are [SemVer](https://semver.org/).

## [0.3.0] — 2026-10-24

### Added

- **A real test suite for the protocol.** 39 tests, no mocks: the wire format (round-trip, byte
  counts for UTF-8, truncated frames, trailing bytes, attribute quoting), the rules grammar, the
  opener chains, and seven end-to-end tests that start the actual `plumberd` binary and talk to it
  over a real socket.
- **Cross-platform opener chains.** `edit` now tries `code -g`, `zed`, `gvim`/`vim`/`nvim +{line}`,
  `subl`, `kate --line`, `micro` and `notepad` in that order; `web` tries `xdg-open`, `open`, then
  Windows `start`; `pdf` tries `zathura`, `pdfcraft`, `okular`, `evince`, `xdg-open`, `open`,
  `start`. The first program found in `PATH` wins, so one binary works on every desktop.
- **A multi-page manual** at <https://kostyk348.github.io/plumber/> — install, the rules grammar
  with a cookbook verified by actually running `plumb test`, the line protocol with shell and Python
  clients, and system integration (user service, MIME handlers, editor hooks).
- **CI on Linux, Windows and macOS**: `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test --all-targets`, plus release archives for four targets on a tag.

### Fixed

- **A malformed frame could panic the daemon's connection thread.** `Message::from_wire` sliced
  `data` by byte count with `&rest[..ndata]`, which panics when the cut lands inside a multi-byte
  character — any client sending a wrong `ndata` with UTF-8 text could kill the connection silently.
  Slicing is now on bytes with lossy decoding.
- **A stalled sender could hold a thread forever.** The daemon now sets a 5-second read timeout per
  read call: a client that says `SEND 4096` and stops sending gets an error instead of pinning a
  connection thread.
- **Frame sizes are bounded** (16 MiB). A frame announcing a gigabyte is refused before any
  allocation.
- **Malformed frames are answered, not dropped.** `ERR malformed message: …` replaces the previous
  behaviour of closing the connection with no reply, which made a client bug look like a daemon
  crash.
- **`vim +{line} {file}` with no line number** no longer becomes `vim + file` ("jump to the last
  line"): the lone `+` is dropped, the same way a dangling `:` already was.

### Changed

- `plumb test` output is documented as what it is — the full message the reader would get — in the
  manual, along with the three traps that bite everyone: `isfile` fails on files that do not exist,
  rule order decides between `manual.pdf:7` and "a file called manual.pdf:7", and an empty capture
  produces an empty attribute.

## [0.2.0] — earlier

- `plumber-open`: a reader that opens what arrives, with `--cmd`, `--dry-run`, `--once`.
- The `page.pdf:12` rule that carries a page number as an attribute.
- A hold queue: a message for a port nobody reads yet waits instead of being refused.
