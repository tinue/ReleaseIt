# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This project may contain
breaking changes in every release, major or minor, until version 1.0.0 is reached
(the C ABI and the Rust API included); from 1.0.0 on it follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`bin/release` reads the section for the version being released and uses it as the
GitHub release notes, so keep entries user-facing.

## [Unreleased]

## [0.3.6] - WIP

### Changed

- Recommended PC-1600 setting for sending is now `SNDSTAT "COM1:",24,0` (was `28,0`):
  the PC-1600 then waits for the PC's RTS before each byte, so RTS/CTS works in both
  directions. With `--no-flowcontrol` (cables without RTS/CTS) keep `SNDSTAT "COM1:",28,0`.
  `RCVSTAT "COM1:",28,0` is unchanged.

## [0.3.5] - 2026-10-10

### Changed

- **`--device pc1600` uses RTS/CTS hardware flow control by default again** and sends
  at full speed, throttled by the PC-1600's RTS. The `ERROR 142` that led to the change
  in 0.2.2 was a cable problem (RTS/CTS not wired or not inverted), not a flow-control
  problem. Recommended PC-1600 settings are now `RCVSTAT "COM1:",28,0` and
  `SNDSTAT "COM1:",28,0`: `RCVSTAT` is only a filter, so `28` is right with RTS/CTS too,
  and the `0` turns off the timeout behind `ERROR 142`. A bad cable now shows up as a
  hang; see the README's troubleshooting section. `pc1600emul` and the PC-1500 family are
  unchanged.

### Added

- `--no-flowcontrol` for `get` and `put` (`pc1600` only): no RTS/CTS, paced transfer as
  for `pc1600emul`. The fallback for cables without working RTS/CTS lines.
- `put` fails with a clear message when the PC-1600 never raises CTS, instead of
  reporting success with the data still stuck in the serial buffer.

### Removed

- `--flowcontrol`. It is the default for `pc1600` now; on `pc1600emul` it had no effect.

## [0.3.4] - 2026-10-07

### Fixed

- **PC-1600 `MODE` is tokenized again.** The PC-1600 keyword table spelled token F2B3H
  `MOOE`, so a listing line `10 MODE 0` became `10 MOD E0`, and F2B3H listed as `MOOE`.
  `WIDTH` (F087H) is gone from the PC-1600 table: a real PC-1600 cannot type it (the
  ROM has it only as a masked entry). Both checked against the PC-1600 ROM's own token
  table.

## [0.3.3] - 2026-10-04

### Added

- **Cassette WAV files** for the PC-1500 / PC-1500A with CE-150 and the PC-1600 with
  CE-1600P (MODE 0): BASIC programs, machine code and the PC-1500 Reserve Area.
  - Reading needs no option: a WAV is recognized by its content. `sde info tape.wav`
    names the tape format and every file on it; `sde get tape.wav` and
    `sde convert tape.wav` write the files as a received file would be (`tape.wav:NAME`
    picks one); `sde put tape.wav` sends it over serial or onto a floppy image.
  - Decoding is tolerant — 5 kHz to 192 kHz, 8 to 32 bit or float, mono or stereo,
    quiet, noisy, inverted, ±25 % speed, wow and flutter — and safe: a file is only
    accepted when all of its checksums match; damaged ones are reported with their
    position on the tape.
  - `-f wav` writes a WAV: `sde convert prog.bas -f wav`, `sde get -f wav` (serial or
    floppy image), with `--name`, `--sample-rate` (16000 to 192000, default 48000; below
    16 kHz the PC-1600's `CLOAD` fails) and `--leader` (default about 2 s on the PC-1500
    and 3 s on the PC-1600 instead of the ROMs' 8 s / 3.3 s; `rom` for the original
    length).
  - `sde put <file> -f wav` plays the tape through the computer's default audio output,
    to load programs into a pocket computer with a CE-150 / CE-1600P but no CE-158.
  - C ABI: `sde_wav_count`, `sde_wav_decode`, `sde_wav_encode`; `sde_file_kind` reports
    `wav-pc1500`, `wav-pc1600` and `wav`. See library.md.
- `sde put <file>... <directory>` stores files in a folder as on a PC-1600 disk:
  BASIC tokenized as `NAME.BAS`, 8.3 names, `-f ascii`, `--force`, `--dry-run`. It
  prepares a folder for Calc-U-1600's host drive (**Mount Directory…**, `S3:`), whose
  `LOAD` expects tokenized BASIC.

### Changed

- **One file-naming convention.** Files sde writes are named the same way whether they
  come from serial, a disk image, a tape or `convert`: `.bas` / `.bbas` for a BASIC
  listing / tokenized BASIC, `.sdar` / `.bsdar` and `.sdav` / `.bsdav` for Reserve Area
  and Variables, `.bin` for machine code only. On a PC-1600 disk BASIC stays `NAME.BAS`
  in both forms, as the PC-1600 saves it.
  - **Breaking:** `convert` writes tokenized BASIC as `.bbas` (was `.bbin`). Old
    `.bbin` files still load.
  - `get -f binary` of BASIC writes `.bbas`; it was `.bas` over serial and `.bin` from a
    disk image or tape. Binary Reserve Area / Variables files are `.bsdar` / `.bsdav`
    (were `.sdar` / `.sdav` over serial and `.bin` from tape).
  - Input extensions are no longer checked: `convert` of a `.bas` file holding tokenized
    BASIC (or a `.bbas` holding a listing) goes by its content instead of failing; `-v`
    mentions the mismatch. When the listing of a tokenized `PROG.bas` would overwrite
    it, the input is renamed to `PROG.bbas` first.

- On Linux, `sde` now uses ALSA for the audio output: building it needs the
  development package (`libasound2-dev`), running it the ALSA library (`libasound2`,
  present on desktop systems). The library build (`--no-default-features`) is
  unchanged.

## [0.3.2] - 2026-09-30

### Changed

- `sde put` of a file with a PC-1600 header (BASIC or machine code) now uses
  `--device pc1600` automatically instead of asking for `--device`; an explicit
  `pc1500`/`pc1500a` is overridden with a warning rather than rejected.
  `--device pc1600emul` is still honored.
- **Breaking:** `--device pc1600emul` now looks for `calcu1600-rs232c.serial`
  (was `calcu1600.serial`). Calc-U-1600 0.7.0 names its serial port files after
  their connector, and this is the PC-1600's RS-232C port. A configured
  `pc1600emul.port` directory stays valid. With an older Calc-U-1600, pass the
  full path instead: `--port <dir>/calcu1600.serial`.

## [0.3.1] - 2026-09-27

### Added

- `sde convert` adds or strips a machine-code header offline:
  `sde convert prog.bin --start-address 38C5 [--run-address …]` writes
  `prog.ce158.bin` (or `prog.pc1600.bin` with `-d pc1600`), and
  `sde convert prog.ce158.bin` writes the bare code to `prog.pure.bin`. A headerless
  file without `--start-address` is an error.
- `sde info <file>` describes a file: BASIC (tokenized or listing), machine code,
  Reserve Area, Variables or text, with its header, name, load/run addresses, sizes and
  line numbers, and warns about truncated or padded files. For machine code without a
  header it guesses the CPU (LH5801 or Z80), labelled as a guess; `-v` shows why.
- Library: `sde_file_kind` returns what a buffer holds as a stable one-word token
  (`basic-pc1500`, `ml-lh5801`, `ml-z80`, `raw-z80`, `text`, …, or `damaged`), and
  `sde_file_info` fills an `SdeFileInfo` with that token, problem flags
  (`SDE_PROBLEM_*`), the payload's offset and length, the load/run address and the
  name, so a program loader needn't parse headers. Rust: `sharpdx::classify`,
  `FileKind`, `FileSummary`. `sde info` shows the same token on its `kind:` line.
- `sde convert -v` now explains each step (detected content, header found or added,
  addresses, bytes written); before, it printed nothing extra.

### Changed

- `sde convert` refuses to write its output over its input file.

### Fixed

- macOS: release builds are stripped by Apple's linker instead of rustc's own strip
  step, whose output macOS 27 refuses to load ("mis-aligned LINKEDIT string pool").
  This made `cargo build --release` fail on macOS 27 and left `libsharpdx.dylib`
  unloadable there.

## [0.3.0] - 2026-09-22

### Changed

- macOS builds (`sde`, `libsharpdx.a`, `libsharpdx.dylib`) now require **macOS 15.8 or
  later**, and the `.pkg` installer refuses to install on older versions. Apps that
  link `libsharpdx` should target macOS 15.8 or later too. (Earlier releases were
  listed as macOS 12+ but were really built for macOS 11, and their installer did not
  check the OS version.)

## [0.2.3] - 2026-09-22

### Added

- Files on Calc-U-1600 CE-1600F floppy images (`<name>.floppy.yaml`):
  `sde dir <image>[:A|:B]` lists a disk, `sde get <image>:<side>:<NAME|pattern>`
  and `sde put <file>... <image>:<side>:[NAME]` copy files off and onto it with the
  same conversions as the serial transfer (BASIC tokenized / de-tokenized, text
  converted, machine code wrapped with `--start-address`), and
  `sde del <image>:<side>:<NAME|pattern>...` deletes files. `put --force` replaces an
  existing file. Checked against the PC-1600 ROM in Calc-U-1600: it loads what sde
  writes, and sde reads what it saves.
- Plain text (assembler source, config files, …) is a recognized content type:
  converted between UTF-8 and the Sharp character set with CRLF line ends and a
  trailing `1A`, for disk images and for serial `get`/`put` on the PC-1600.
- `get --eol auto|lf|crlf|cr` sets the line ending of a listing or text file.
- `put -f binary` on input detected as text tokenizes it as a BASIC listing anyway —
  the override when a listing isn't recognized (C ABI: `SDE_DISK_PUT_MODE_TOKENIZE`).
- C ABI: `sde_disk_list`, `sde_disk_get`, `sde_disk_put`, `sde_disk_delete` work on
  one floppy side in memory, with new error codes `SDE_ERR_NOT_FOUND` …
  `SDE_ERR_DIRECTORY_FULL`. See `library.md`.

### Changed

- `sde_detect` / `sde_convert` report plain text as the new `SDE_CONTENT_TEXT`
  instead of `SDE_CONTENT_UNKNOWN`.
- `get` of a headerless ASCII listing now honours `--eol` (default on Windows is
  therefore CRLF, was LF); a received binary with no recognizable content is saved
  as `unnamed.bin` instead of `unnamed.bas`.

- Stricter test for ASCII BASIC: a line counts as a listing line only with at most 4
  leading spaces, a line number, exactly one space or tab, and the statement. A data
  file starting with a number (e.g. a high-score file `      50        1 MARTIN`) is
  now text instead of BASIC, so `put` no longer tokenizes it by mistake.

### Fixed

- A short ASCII listing ending in the end-of-file byte `1A` is recognized as BASIC.
- The `sde_tokenize` calls in `library.md` and `examples/` pass the
  `segment_marker` argument added in 0.2.2.

## [0.2.2] - 2026-09-20

### Added

- `--flowcontrol` option for `get` and `put` (`pc1600` / `pc1600emul` only)
  enables RTS/CTS hardware flow control. On `pc1600emul` it is a best-effort
  attempt that probably has no effect.

### Changed

- `pc1600` no longer uses RTS/CTS flow control by default and paces the transfer
  like `pc1600emul`, which avoids the frequent `ERROR 142`. On the PC-1600 use
  `RCVSTAT "COM1:",28` and `SNDSTAT "COM1:",28` (was `24`); with `--flowcontrol`,
  keep `24` (only `RCVSTAT` matters for `put`).
- Hardware notes (adapter photos, serial cable and flow-control hints) moved into
  `docs/HardwareNotes.md`, linked from the README; the Apple Silicon RTS/CTS note
  and its Raspberry Pi workaround are gone, as RTS/CTS is no longer the default.

### Fixed

- `get --raw` on a headerless capture (e.g. a ROM dump) no longer drops 16 bytes
  when the data happens to contain the PC-1600 header magic `FF 10 00 00`. A
  header is now only recognized at the start of the received data (optionally
  after `0x00` noise), for both `--raw` and normal `get`.

## [0.2.1] - 2026-09-19

### Changed

- The libraries in release archives (`lib/`) are now built without the serial
  transport, so embedders no longer link `serialport` or, on Windows, its extra
  import libs (e.g. `windows.0.52.0.lib`). The C ABI is unchanged.
- New Cargo features: `serial` (`get`/`put` transport) and `cli` (the `sde`
  binary, implies `serial`), both on by default. Build a serial-free lib with
  `cargo build --release --lib --no-default-features`.
- Windows arm64 release builds now run natively on an arm64 runner and run the
  test suite before packaging.
- The GitHub repository is now `tinue/SharpDataExchange` (formerly
  `SharpDataExchangeRust`; old URLs redirect). Release, changelog and
  `library.md` links point to the new name.

## [0.2.0] - 2026-09-18

### Added

- `sde get` / `sde put`: Reserve Area (SDAR ASCII form, key-assignment layers)
  and Variables (SDAV ASCII form) transfer support for PC-1500/PC-1500A.
  Both are get/put-only and PC-1500-only — `convert` stays BASIC-only and the
  C ABI keeps collapsing them to `Unknown`. Adds a PC-1500 8-byte BCD numeric
  codec (`src/bcd.rs`) to encode/decode Variables' numeric records.
- `install.sh`: builds `sde` in release mode and installs it to
  `~/Applications` on macOS.

### Fixed

- Windows release archives: `sharpdx.lib` is now shipped alongside any
  transitively-linked import lib that isn't a standard Windows SDK/CRT lib
  (e.g. an older `windows-sys`'s classic import lib, pulled in by a
  dependency's Windows backend). Without it, a downstream consumer linking
  the raw static lib directly hit "cannot open input file" for a name
  `native-static-libs` listed but never supplied.

## [0.1.5] - 2026-09-18

### Added

- `sde get` / `sde put`: real serial transfer of BASIC and machine-language
  programs to/from a real or emulated Sharp PC-1500 / PC-1500A / PC-1600, on top
  of the existing offline `convert` verb. Supports all four transport targets
  (`pc1500`/`pc1500a`/`pc1600`/`pc1600emul`), `--raw`, `--dry-run`, and
  header auto-add/ambiguity handling. Reserve Area and Variables transfer remain
  out of scope.
- `sde config`: reads an optional `~/.sderc` (`key = value`) for two defaults --
  the `pc1600emul` socket directory (fixed filename `calcu1600.serial`, default
  `/tmp`) and default verbosity -- plus `-v`/`-q` CLI overrides shared by `get`
  and `put`.
- PC-1600 patches a constant `GOTO`/`GOSUB`/bare-`THEN` jump target into a
  compact binary form (`0x1F [hi] [lo] 0x00`) instead of leaving it as ASCII
  digits (PC-1500 always uses ASCII digits). Confirmed against real
  PC-1500/PC-1600 memory dumps; `scanner::tokenize` reproduces this on PC-1600
  only, `detokenize::detokenize` decodes it on both devices.
- The device supports multiple `GOSUB "LABEL"`-addressable program segments per
  save, separated on the wire by `0xFF 0x00 0x00`. Confirmed on real PC-1600
  hardware. The reserved `#SEGMENT` marker line now round-trips this boundary in
  both directions; `sde_tokenize` (and `convert_with`/`scanner::tokenize`) take
  a `SdeSegmentMarker` option controlling how it tokenizes: `Wire`
  (`0xFF 0x00 0x00`, what `SAVE "COM1:"` actually transmits, the previous and
  default behavior) or `Memory` (bare `0xFF`, what the ROM's serial receiver
  actually stores into the program area). A caller poking tokenized bytes
  directly into RAM -- bypassing the serial protocol entirely -- should use
  `Memory`, since including the two wire-only pacing bytes made `BASPRG_END`
  come out 2 bytes too high per marker.

### Fixed

- `header::build_pc1600`'s end-of-header marker was `{0x00, 0xF0}`; a real
  PC-1600 capture confirms it should be `{0x00, 0x0F}`. Fixed, with the
  `depreciation-tokenized-pc1600header.bin` fixture regenerated to match.

## [0.1.4] - 2026-09-16

### Fixed

- PC-1600 keyword table: added native `PEEK`, `PEEK#`, `POKE`, and `CALL` tokens
  (0xF26D, 0xF26E, 0xF28C, 0xF282), distinct from `XPEEK`/`XPEEK#`/`XPOKE`/`XCALL`,
  which tokenize the PC-1500's codes for those same names.
- PC-1600 keyword table: removed the spurious `PROTOCOL` entry, which is a CE-158
  extension keyword, not a native PC-1600 one.
- PC-1600 keyword table: listed `DEV$` and `COM$` explicitly (same codes as
  CE-158's), since they are native to the PC-1600's built-in serial port.

## [0.1.3] - 2026-09-11

### Fixed

- `bin/release --release` crashed with `MAIN_BRANCH: unbound variable` while
  tagging the release commit: this bash mis-parses a bare `$VAR` immediately
  followed by a multi-byte UTF-8 character (here the `…` right after
  `$MAIN_BRANCH`) as part of the variable name. Braced `${MAIN_BRANCH}` fixes
  it; the release commit and tag still land correctly, only the final "no
  rebuild" promotion step was interrupted, so `v0.1.2`'s promotion had to be
  finished by hand.
- The tokenizer discarded a `'` (the `REM` shorthand) instead of keeping it,
  then went on to parse the rest of the line as code — a comment like
  `10 'X1 = 1.POSITION ON STACK` had `POSITION`/`ON` mistaken for keywords.
  `'` now behaves exactly like `REM`: it is kept, and everything after it on
  the line is copied through verbatim. The dotted-abbreviation expander had
  the same gap (`'CALL P. HERE` was corrupted into `'CALL PRINT HERE` before
  the scanner ever saw it) and is fixed the same way.
- `bin/release`'s `publish` job 403'd downloading its own build artifacts
  ("Failed to ListArtifacts") — a side effect of the earlier
  `download-artifact@v4->v7` bump: v5+ lists/downloads artifacts via the
  Actions REST API, which needs `actions: read`, but the job's `permissions:`
  block only granted `contents: write`. Added `actions: read`.

## [0.1.2] - 2026-09-11

### Fixed

- `lib/native-libs-windows.txt` (added in 0.1.1) was corrupted by a stray ANSI
  color-reset escape (`CARGO_TERM_COLOR=always` in CI coloring a note captured
  from a redirected, non-tty stderr) and its doc comment wrongly described the
  content as GCC `-lname` syntax; it's actually MSVC linker tokens (bare
  `name.lib` filenames and the occasional `/defaultlib:x` flag). Packaging now
  captures it with `CARGO_TERM_COLOR=never` and strips escapes defensively.

## [0.1.1] - 2026-09-11

### Added

- Windows release archives now include `lib/native-libs-windows.txt`: the Windows
  system import libs (`ws2_32`, `userenv`, `bcrypt`, ...) a non-cargo consumer
  linking the raw `sharpdx.lib` needs to supply itself, captured straight from
  `rustc --print=native-static-libs` for that build rather than left for every
  downstream consumer to guess.
- Platform-aware line endings for de-tokenized listings. The output now uses the
  host convention by default — `CRLF` on Windows, `LF` on macOS / Linux — and the
  terminator is overridable: `--eol auto|lf|crlf|cr` on the CLI,
  `sharpdx::convert_with(.., LineEnding)` in the crate, and a new
  `SdeLineEnding line_ending` argument on `sde_detokenize` / `sde_convert` in the
  C ABI.
- macOS releases are signed with a Developer ID and notarized (hardened runtime),
  so they run without a Gatekeeper warning. The `sde` / `libsharpdx.dylib` in the
  `.tar.gz` are notarized; a new **`SharpDataExchange-<version>.pkg`** installer is
  published alongside it — signed, notarized, and stapled — with a choice between
  an all-users install (`/usr/local/bin`) and a per-user one (`~/.local/bin`,
  added to `~/.zshrc`).

### Changed

- Tokenizing now accepts `CR` and `CRLF` line endings in the input listing on
  every platform (previously a real Windows `.bas` file with `CRLF` endings could
  leave a stray `0x0D` in the payload).
- **C ABI break:** `sde_detokenize` and `sde_convert` take an extra
  `SdeLineEnding line_ending` parameter (pass `SDE_LINE_ENDING_PLATFORM` for the
  previous-style host default). `sde_tokenize` is unchanged.

## [0.1.0] - 2026-09-08

### Added

- ROM-style linear scanner (`src/scanner.rs`) that tokenizes Sharp PC-1500 /
  PC-1600 BASIC in one left-to-right pass with a 3-state (normal / string /
  verbatim) model, instead of a grammar.
- `sde convert` CLI: content-detected direction, `-d/--device`
  `pc1500|pc1500a|pc1600|pc1600emul`, file or stdin/stdout I/O, output-path
  derivation.
- `libsharpdx` C ABI (`sde_convert`, `sde_tokenize`, `sde_detokenize`,
  `sde_detect`, `sde_version`, `sde_last_error`, `sde_buf_free`) with a
  panic-free boundary and caller-freed buffers. Committed C header and Swift
  module map generated from `src/ffi.rs`.
- `sharpdx` Rust crate exposing the pure `convert` core.
- Byte-parity test suite against the Java `SharpDataExchange` `convert` fixtures,
  plus round-trip and C-ABI tests.
- Release archives for macOS (Apple Silicon), Linux x86-64 / arm64, and Windows
  x64 / arm64, each bundling the CLI, the static and shared libraries, and the
  `include/` directory.

### Known limitations

- `convert` only — no `get` / `put` / `terminal`, serial I/O, Variables /
  Reserve-Area conversion, or cassette headers.
- PC-1600-only token values are limited to what `Pc1600Keywords.java` lists;
  unknown `>= 0xE0` byte pairs pass through opaquely.
- The CE-158 header filename field is upper-cased (the historical fixture stored
  it lower-case); `testsuite.md` marks those bytes "don't care". Payloads match
  the Java output exactly.

[Unreleased]: https://github.com/tinue/SharpDataExchange/compare/v0.3.6...HEAD
[0.3.6]: https://github.com/tinue/SharpDataExchange/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/tinue/SharpDataExchange/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/tinue/SharpDataExchange/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/tinue/SharpDataExchange/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/tinue/SharpDataExchange/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/tinue/SharpDataExchange/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/tinue/SharpDataExchange/compare/v0.2.3...v0.3.0
[0.2.3]: https://github.com/tinue/SharpDataExchange/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/tinue/SharpDataExchange/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/tinue/SharpDataExchange/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/tinue/SharpDataExchange/compare/v0.1.5...v0.2.0
[0.1.5]: https://github.com/tinue/SharpDataExchange/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/tinue/SharpDataExchange/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/tinue/SharpDataExchange/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/tinue/SharpDataExchange/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/tinue/SharpDataExchange/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/tinue/SharpDataExchange/releases/tag/v0.1.0
