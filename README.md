# Echo Scribe

Echo Scribe is a local-first desktop app for recording audio and generating transcripts on-device.

## v0.2 Highlights

- Bundled `whisper-cli` sidecar for release builds (macOS arm64)
- In-app model download with checksum verification
- Markdown transcript output with frontmatter metadata
- Optional CoachNotes mode (`<CoachRoot>/<Client>/<date>-transcript-<time>.md`)
- Experimental 2-speaker mode (English only, `small.en-tdrz` + `-tdrz`)

## Runtime Model Setup

The app downloads Whisper model files into app data (`models/`) and verifies SHA-256 checksums.

Supported models:

- `tiny`
- `base`
- `small`
- `medium`
- `small.en-tdrz` (experimental diarization)

## Transcript Output Format

Saved transcripts are Markdown with YAML frontmatter:

```md
---
title: "Session Transcript"
date: "YYYY-MM-DD"
client: "Client Name"
source_app: "Echo Scribe"
created_at: "ISO-8601"
model: "base|small|medium|small.en-tdrz"
language: "auto|en|..."
diarization_mode: "none|tdrz_2speaker"
duration_seconds: 0
---
# Transcript

...content...
```

## CoachNotes Mode

CoachNotes mode is optional.

When enabled:

- You choose a CoachNotes root folder.
- Echo Scribe reads first-level subfolders as client names (excluding hidden folders and `Deleted Notes`).
- You choose the client from a dropdown.
- Saved transcript path: `<CoachRoot>/<Client>/<YYYY-MM-DD>-transcript-<HHmmss>.md`

## Experimental 2-Speaker Mode

The 2-speaker mode is best-effort and currently constrained to:

- Language: `en`
- Model: `small.en-tdrz`

If constraints are not met, Echo Scribe falls back to standard transcription and reports a warning.

## Development

```bash
npm install
npm run tauri dev
```

In debug mode, if sidecar is unavailable, Echo Scribe attempts local fallback discovery for whisper binaries.

## Release / CI

The GitHub workflow builds macOS arm64 only and compiles `whisper-cli` from pinned `ggml-org/whisper.cpp` source, then bundles it as a Tauri sidecar.
