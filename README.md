# webvtt-to-md

High-performance Rust WebVTT to Markdown converter.

## What It Does

- Converts `.vtt` transcripts to Markdown speaker blocks.
- Auto-detects transcript generation source (Teams/Zoom/generic patterns).
- Supports strict and lazy decoding modes for mixed-quality input encodings.

## Build

```bash
cargo build --release
```

Binary path after build:

```bash
./target/release/webvtt-to-md
```

## CLI Usage

```bash
webvtt-to-md [OPTIONS] <INPUT>
```

`<INPUT>` is required and must point to a `.vtt` file.

### Options

- `-o, --output <OUTPUT>`
  - Write Markdown to an output file.
  - If omitted, Markdown is written to stdout.
- `--charset <CHARSET>`
  - Force a specific input charset (examples: `utf-8`, `utf-16le`, `cp1251`).
- `--charset-lazy`
  - Lazy decoding mode.
  - Replaces undecodable bytes with `[ENC?]` instead of returning an error.
- `--source <SOURCE>`
  - Force generation-source ruleset instead of auto-detection.
  - Allowed CLI values:
    - `generic-webvtt`
    - `ms-teams-scheduled`
    - `ms-teams-event`
    - `zoom-scheduled`
    - `zoom-event`
    - `meeting-novoicetag`
- `--detect-source`
  - Print detected source type and exit (no markdown output).
- `-h, --help`
  - Show help.

## Common Examples

### 1) Convert to stdout

```bash
cargo run --release -- Blueprint_way_forward.vtt
```

### 2) Convert to file

```bash
cargo run --release -- Blueprint_way_forward.vtt -o Blueprint_way_forward.md
```

### 3) Detect source only

```bash
cargo run --release -- Blueprint_way_forward.vtt --detect-source
```

### 4) Force a source

```bash
cargo run --release -- Blueprint_way_forward.vtt --source generic-webvtt -o out.md
```

### 5) Strict charset override (fail on decode errors)

```bash
cargo run --release -- Blueprint_way_forward.vtt --charset utf-8 -o out.md
```

### 6) Lazy charset mode (replace invalid bytes)

```bash
cargo run --release -- Blueprint_way_forward.vtt --charset utf-8 --charset-lazy -o out.md
```

## Decoding Behavior

Default behavior is strict.

When `--charset` is not provided, the converter attempts:

- BOM-aware decoding (`UTF-8`, `UTF-16`, `UTF-16BE`, `UTF-32` BOMs)
- UTF-16 heuristic detection for BOM-less UTF-16 data
- UTF-8 strict decode
- charset detection fallback
- fallback legacy encodings (`cp1254`, `cp1251`, `cp1252`)

With `--charset-lazy`, undecodable sequences are replaced with `[ENC?]`.

## Test Fixtures

This repo includes a regression fixture and test for:

- input: `Blueprint_way_forward.vtt`
- expected markdown: `tests/fixtures/converted_blueprint_way_forward.md`
- test: `blueprint_way_forward_fixture_matches` in `tests/converter_integration.rs`

Run tests:

```bash
RUSTC_WRAPPER= cargo test
```
