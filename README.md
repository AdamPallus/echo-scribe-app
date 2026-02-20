# Echo Scribe

Echo Scribe is a local-first desktop app for recording audio and generating transcripts on-device.

## v0.2 Highlights

- Bundled `whisper-cli` sidecar for release builds (macOS arm64)
- In-app model download with checksum verification
- Markdown transcript output with frontmatter metadata
- Optional CoachNotes mode (`<CoachRoot>/<Client>/<date>-transcript-<time>.md`)
- Experimental 2-speaker mode (English only, `small.en-tdrz` + `-tdrz`)
- Capture mode selector: microphone, system audio, or system+microphone

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

Standard mode (`CoachNotes mode` off):

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

CoachNotes mode (`CoachNotes mode` on):

```md
---
client: "Client Name"
date: "YYYY-MM-DD"
title: "Session Transcript"
note_type: "transcript"
source: "coachnotes-voice-app"
transcript: true
speakers:
  - "Coach"
  - "Client"
tags:
  - "transcript"
  - "coaching-session"
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

You can re-run transcription on the same recording by changing model/language/speaker mode and pressing `Transcribe Again`.

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

When enabled in the UI, Echo Scribe auto-switches to English + `small.en-tdrz`.

## Development

```bash
npm install
npm run tauri dev
```

In debug mode, if sidecar is unavailable, Echo Scribe attempts local fallback discovery for whisper binaries.

## System Audio Capture Notes

- Use `Capture Source` in the Recorder section to choose `System audio only` or `System audio + microphone`.
- System audio capture uses a native macOS sidecar (ScreenCaptureKit) and does not require picking a window.
- On first use, macOS asks for `Screen Recording` permission (and `Microphone` permission if mic mode is also enabled).
- If permission is denied, open `System Settings > Privacy & Security` and enable access for Echo Scribe.
- Default capture mode is `System audio + microphone`.
- Default language is `English`.

## Release / CI

The GitHub workflow builds macOS arm64 only, compiles `whisper-cli` plus the native `system-audio-capture` sidecar, then bundles both into the app.
