use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_shell::ShellExt;
use time::{format_description::well_known::Rfc3339, macros::format_description, OffsetDateTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COACHNOTES_DELETED_DIR: &str = "Deleted Notes";
const SPEAKER_TURN_MARKER: &str = "[SPEAKER_TURN]";
const SYSTEM_AUDIO_CAPTURE_PLACEHOLDER_MARKER: &str =
    "system-audio-capture sidecar placeholder";
const BLANK_AUDIO_MARKER: &str = "[BLANK_AUDIO]";

#[derive(Debug, Clone, Copy)]
struct ModelCatalogEntry {
    id: &'static str,
    label: &'static str,
    size_mb: u32,
    url: &'static str,
    sha256: &'static str,
}

const MODEL_CATALOG: [ModelCatalogEntry; 5] = [
    ModelCatalogEntry {
        id: "tiny",
        label: "Tiny (fastest, lowest accuracy)",
        size_mb: 75,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
    },
    ModelCatalogEntry {
        id: "base",
        label: "Base (recommended on MacBook Air)",
        size_mb: 142,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    },
    ModelCatalogEntry {
        id: "small",
        label: "Small (higher quality)",
        size_mb: 466,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    },
    ModelCatalogEntry {
        id: "medium",
        label: "Medium (best quality, slower)",
        size_mb: 1500,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
    },
    ModelCatalogEntry {
        id: "small.en-tdrz",
        label: "Small.en + tdrz (experimental 2-speaker, English)",
        size_mb: 466,
        url: "https://huggingface.co/akashmjn/tinydiarize-whisper.cpp/resolve/main/ggml-small.en-tdrz.bin",
        sha256: "ceac3ec06d1d98ef71aec665283564631055fd6129b79d8e1be4f9cc33cc54b4",
    },
];

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppSettings {
    selected_model: String,
    transcript_dir: Option<String>,
    transcript_format: String,
    coachnotes_enabled: bool,
    coachnotes_root_dir: Option<String>,
    coachnotes_client: Option<String>,
    diarization_mode: String,
    #[serde(default)]
    diarization_mode_configured: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            selected_model: "base".to_string(),
            transcript_dir: None,
            transcript_format: "md".to_string(),
            coachnotes_enabled: false,
            coachnotes_root_dir: None,
            coachnotes_client: None,
            diarization_mode: "source_aware_2speaker".to_string(),
            diarization_mode_configured: false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ModelState {
    id: String,
    label: String,
    size_mb: u32,
    downloaded: bool,
    path: String,
}

#[derive(Debug, Serialize)]
pub struct DiarizationCapabilities {
    tdrz_english_only: bool,
}

#[derive(Debug, Serialize)]
pub struct SetupState {
    selected_model: String,
    transcript_dir: String,
    transcript_format: String,
    models_dir: String,
    models: Vec<ModelState>,
    ready: bool,
    sidecar_ready: bool,
    coachnotes_enabled: bool,
    coachnotes_root_dir: Option<String>,
    coachnotes_clients: Vec<String>,
    coachnotes_client: Option<String>,
    diarization_mode: String,
    diarization_capabilities: DiarizationCapabilities,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TranscriptionOptions {
    audio_data: Vec<u8>,
    #[serde(default)]
    microphone_audio_data: Vec<u8>,
    #[serde(default)]
    system_audio_data: Vec<u8>,
    #[serde(default)]
    system_audio_offset_ms: u64,
    model: String,
    language: String,
    save_markdown: bool,
    #[serde(default)]
    save_raw_audio: bool,
    output_mode: String,
    client: Option<String>,
    diarization_mode: String,
}

#[derive(Debug, Serialize)]
pub struct TranscriptionResult {
    transcript: String,
    saved_path: Option<String>,
    saved_audio_paths: Vec<String>,
    format: String,
    diarization_applied: bool,
    speaker_mode_used: String,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelDownloadOptions {
    model: String,
}

#[derive(Debug, Serialize)]
pub struct ModelDownloadResult {
    model: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CoachNotesSettingsInput {
    enabled: bool,
    root_dir: Option<String>,
    client: Option<String>,
}

#[derive(Clone, Serialize)]
struct ProgressPayload {
    percent: u32,
    message: String,
}

#[derive(Clone, Serialize)]
struct ModelDownloadPayload {
    model: String,
    percent: u32,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: String,
}

struct WhisperOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    used_sidecar: bool,
}

struct SystemAudioCaptureSession {
    child: Child,
    output_path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemAudioCaptureMetadata {
    #[serde(default)]
    first_audio_wall_time_ms: u64,
}

#[derive(Debug, Serialize)]
struct SystemAudioCaptureResult {
    audio_data: Vec<u8>,
    first_audio_wall_time_ms: u64,
}

#[derive(Default)]
struct SystemAudioCaptureState {
    session: Mutex<Option<SystemAudioCaptureSession>>,
}

struct TempFileCleanup {
    paths: Vec<PathBuf>,
}

impl TempFileCleanup {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

impl Drop for TempFileCleanup {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

#[derive(Clone, Copy)]
enum WhisperFileFormat {
    Txt,
    Srt,
}

impl WhisperFileFormat {
    fn cli_flag(self) -> &'static str {
        match self {
            Self::Txt => "-otxt",
            Self::Srt => "-osrt",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Srt => "srt",
        }
    }
}

struct WhisperTranscriptOutput {
    content: String,
    used_sidecar: bool,
}

#[derive(Clone)]
struct TimestampedSegment {
    speaker: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Clone)]
struct WordSpan {
    start: usize,
    end: usize,
    normalized: String,
}

fn emit_progress(app: &AppHandle, percent: u32, message: &str) {
    let _ = app.emit(
        "progress",
        ProgressPayload {
            percent,
            message: message.to_string(),
        },
    );
}

fn emit_model_download_progress(
    app: &AppHandle,
    model: &str,
    percent: u32,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: &str,
) {
    let _ = app.emit(
        "model-download-progress",
        ModelDownloadPayload {
            model: model.to_string(),
            percent,
            downloaded_bytes,
            total_bytes,
            message: message.to_string(),
        },
    );
}

fn sanitize_non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn now_local_or_utc() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

fn format_date(now: OffsetDateTime) -> String {
    now.format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_else(|_| "1970-01-01".to_string())
}

fn format_time_compact(now: OffsetDateTime) -> String {
    now.format(format_description!("[hour][minute][second]"))
        .unwrap_or_else(|_| "000000".to_string())
}

fn format_iso8601(now: OffsetDateTime) -> String {
    now.format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn unix_timestamp_secs() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|e| format!("System clock error: {}", e))
}

fn find_model(model_id: &str) -> Option<&'static ModelCatalogEntry> {
    MODEL_CATALOG.iter().find(|entry| entry.id == model_id)
}

fn validate_model(model_id: &str) -> Result<&'static ModelCatalogEntry, String> {
    find_model(model_id).ok_or_else(|| {
        format!(
            "Unsupported model '{}'. Valid values: {}",
            model_id,
            MODEL_CATALOG
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<&str>>()
                .join(", ")
        )
    })
}

fn validate_output_mode(mode: &str) -> &'static str {
    match mode {
        "coachnotes" => "coachnotes",
        _ => "standard",
    }
}

fn validate_diarization_mode(mode: &str) -> &'static str {
    match mode {
        "source_aware_2speaker" => "source_aware_2speaker",
        "tdrz_2speaker" => "tdrz_2speaker",
        _ => "none",
    }
}

fn echo_scribe_temp_dir() -> Result<PathBuf, String> {
    let temp_dir = std::env::temp_dir().join("echo-scribe");
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temporary directory: {}", e))?;
    Ok(temp_dir)
}

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {}", e))
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("models"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("settings.json"))
}

fn default_transcript_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("EchoScribe Transcripts")
}

fn resolve_transcript_dir(settings: &AppSettings) -> PathBuf {
    if let Some(path) = &settings.transcript_dir {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    default_transcript_dir()
}

fn model_file_path(app: &AppHandle, model: &str) -> Result<PathBuf, String> {
    Ok(models_dir(app)?.join(format!("ggml-{}.bin", model)))
}

fn load_settings(app: &AppHandle) -> Result<AppSettings, String> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read settings file ({}): {}", path.display(), e))?;

    let mut settings: AppSettings =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid settings JSON: {}", e))?;

    if find_model(&settings.selected_model).is_none() {
        settings.selected_model = AppSettings::default().selected_model;
    }
    settings.transcript_format = "md".to_string();
    settings.diarization_mode = validate_diarization_mode(&settings.diarization_mode).to_string();
    if !settings.diarization_mode_configured {
        settings.diarization_mode = AppSettings::default().diarization_mode;
    }
    settings.coachnotes_root_dir = sanitize_non_empty(settings.coachnotes_root_dir.clone());
    settings.coachnotes_client = sanitize_non_empty(settings.coachnotes_client.clone());

    Ok(settings)
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let app_dir = app_data_dir(app)?;
    fs::create_dir_all(&app_dir).map_err(|e| {
        format!(
            "Failed to create app data directory ({}): {}",
            app_dir.display(),
            e
        )
    })?;

    let path = settings_path(app)?;
    let serialized = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;

    fs::write(&path, serialized)
        .map_err(|e| format!("Failed to write settings file ({}): {}", path.display(), e))
}

async fn sha256_for_file(path: &Path) -> Result<String, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read_bytes = file
            .read(&mut buffer)
            .await
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

        if read_bytes == 0 {
            break;
        }

        hasher.update(&buffer[..read_bytes]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn sidecar_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    Some(parent.join("whisper-cli"))
}

#[cfg(debug_assertions)]
fn debug_whisper_fallback_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();

    let local = home.join("whisper.cpp/build/bin/whisper-cli");
    if local.exists() {
        return local;
    }

    let local_old = home.join("whisper.cpp/main");
    if local_old.exists() {
        return local_old;
    }

    let brew = PathBuf::from("/opt/homebrew/bin/whisper-cpp");
    if brew.exists() {
        return brew;
    }

    PathBuf::from("whisper-cli")
}

fn system_audio_capture_sidecar_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("system-audio-capture"));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("binaries/system-audio-capture-aarch64-apple-darwin"));
        candidates.push(cwd.join("src-tauri/binaries/system-audio-capture-aarch64-apple-darwin"));
        candidates
            .push(cwd.join("../src-tauri/binaries/system-audio-capture-aarch64-apple-darwin"));
    }

    candidates.push(PathBuf::from("system-audio-capture"));
    candidates
}

fn file_contains_marker(path: &Path, marker: &str) -> bool {
    fs::read_to_string(path)
        .map(|content| content.contains(marker))
        .unwrap_or(false)
}

#[cfg(debug_assertions)]
fn debug_system_audio_capture_binary_path(app: &AppHandle) -> Result<PathBuf, String> {
    let app_dir = app_data_dir(app)?;
    let dev_dir = app_dir.join("dev-binaries");
    fs::create_dir_all(&dev_dir).map_err(|e| {
        format!(
            "Failed to create dev binary directory ({}): {}",
            dev_dir.display(),
            e
        )
    })?;
    Ok(dev_dir.join("system-audio-capture"))
}

#[cfg(debug_assertions)]
fn debug_system_audio_capture_source_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native/system_audio_capture.swift")
}

#[cfg(debug_assertions)]
fn should_rebuild_debug_binary(source_path: &Path, binary_path: &Path) -> bool {
    if !binary_path.exists() {
        return true;
    }

    let Ok(source_meta) = fs::metadata(source_path) else {
        return false;
    };
    let Ok(binary_meta) = fs::metadata(binary_path) else {
        return true;
    };
    let Ok(source_modified) = source_meta.modified() else {
        return false;
    };
    let Ok(binary_modified) = binary_meta.modified() else {
        return true;
    };

    source_modified > binary_modified
}

#[cfg(debug_assertions)]
fn ensure_debug_system_audio_capture_binary(app: &AppHandle) -> Result<PathBuf, String> {
    let source_path = debug_system_audio_capture_source_path();
    if !source_path.exists() {
        return Err(format!(
            "System audio capture source file is missing: {}",
            source_path.display()
        ));
    }

    let binary_path = debug_system_audio_capture_binary_path(app)?;
    if should_rebuild_debug_binary(&source_path, &binary_path) {
        let output = StdCommand::new("swiftc")
            .args([
                "-O",
                "-parse-as-library",
                "-framework",
                "AVFoundation",
                "-framework",
                "CoreGraphics",
                "-framework",
                "CoreMedia",
                "-framework",
                "ScreenCaptureKit",
            ])
            .arg(&source_path)
            .arg("-o")
            .arg(&binary_path)
            .output()
            .map_err(|e| format!("Failed to launch swiftc for system audio helper: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Failed to build local system audio capture helper: {}",
                process_output_detail(&output.stdout, &output.stderr)
            ));
        }
    }

    Ok(binary_path)
}

fn resolve_system_audio_capture_sidecar_path(_app_handle: &AppHandle) -> Result<PathBuf, String> {
    for candidate in system_audio_capture_sidecar_candidates() {
        if candidate.exists() {
            if file_contains_marker(&candidate, SYSTEM_AUDIO_CAPTURE_PLACEHOLDER_MARKER) {
                #[cfg(debug_assertions)]
                {
                    return ensure_debug_system_audio_capture_binary(_app_handle);
                }

                #[cfg(not(debug_assertions))]
                {
                    continue;
                }
            }

            return Ok(candidate);
        }
    }

    #[cfg(debug_assertions)]
    {
        return ensure_debug_system_audio_capture_binary(_app_handle);
    }

    #[cfg(not(debug_assertions))]
    {
        Ok(PathBuf::from("system-audio-capture"))
    }
}

fn is_sidecar_available() -> bool {
    sidecar_binary_path().map(|p| p.exists()).unwrap_or(false)
}

fn process_output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr_text = String::from_utf8_lossy(stderr).trim().to_string();
    if !stderr_text.is_empty() {
        return stderr_text;
    }

    let stdout_text = String::from_utf8_lossy(stdout).trim().to_string();
    if !stdout_text.is_empty() {
        return stdout_text;
    }

    "process exited without output".to_string()
}

fn list_coachnotes_clients_from_root(root_dir: &Path) -> Result<Vec<String>, String> {
    if !root_dir.exists() {
        return Err(format!(
            "CoachNotes root does not exist: {}",
            root_dir.display()
        ));
    }

    let mut clients = Vec::new();
    let entries = fs::read_dir(root_dir)
        .map_err(|e| format!("Failed to read {}: {}", root_dir.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| format!("Failed to read file type: {}", e))?;
        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == COACHNOTES_DELETED_DIR {
            continue;
        }

        clients.push(name);
    }

    clients.sort_by_key(|name| name.to_ascii_lowercase());
    Ok(clients)
}

async fn run_whisper(app: &AppHandle, args: &[String]) -> Result<WhisperOutput, String> {
    #[cfg(not(debug_assertions))]
    {
        let command = app
            .shell()
            .sidecar("whisper-cli")
            .map_err(|e| format!("Whisper sidecar is unavailable: {}", e))?;

        let output = command
            .args(args)
            .output()
            .await
            .map_err(|e| format!("Failed to execute whisper sidecar: {}", e))?;

        return Ok(WhisperOutput {
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
            used_sidecar: true,
        });
    }

    #[cfg(debug_assertions)]
    {
        let mut sidecar_failure: Option<String> = None;

        if let Ok(command) = app.shell().sidecar("whisper-cli") {
            match command.args(args).output().await {
                Ok(output) => {
                    if output.status.success() {
                        return Ok(WhisperOutput {
                            success: true,
                            stdout: output.stdout,
                            stderr: output.stderr,
                            used_sidecar: true,
                        });
                    }

                    sidecar_failure = Some(format!(
                        "Debug sidecar failed: {}",
                        process_output_detail(&output.stdout, &output.stderr)
                    ));
                }
                Err(error) => {
                    sidecar_failure = Some(format!("Debug sidecar could not run: {}", error));
                }
            }
        }

        let whisper_path = debug_whisper_fallback_path();
        let fallback = StdCommand::new(&whisper_path)
            .args(args)
            .output()
            .map_err(|e| {
                format!(
                    "Failed to run whisper fallback binary ({}): {}",
                    whisper_path.display(),
                    e
                )
            })?;

        if fallback.status.success() {
            return Ok(WhisperOutput {
                success: true,
                stdout: fallback.stdout,
                stderr: fallback.stderr,
                used_sidecar: false,
            });
        }

        let mut detail = format!(
            "Whisper fallback failed ({}): {}",
            whisper_path.display(),
            process_output_detail(&fallback.stdout, &fallback.stderr)
        );
        if let Some(sidecar_failure) = sidecar_failure {
            detail = format!("{} | {}", sidecar_failure, detail);
        }

        Ok(WhisperOutput {
            success: false,
            stdout: detail.into_bytes(),
            stderr: Vec::new(),
            used_sidecar: false,
        })
    }
}

async fn transcribe_with_temp_output(
    app: &AppHandle,
    model_path: &Path,
    wav_data: &[u8],
    language: &str,
    diarization_mode: &str,
    format: WhisperFileFormat,
    stem: &str,
) -> Result<WhisperTranscriptOutput, String> {
    let temp_dir = echo_scribe_temp_dir()?;
    let wav_path = temp_dir.join(format!("{}.wav", stem));
    let output_base = temp_dir.join(stem);
    let transcript_path = temp_dir.join(format!("{}.{}", stem, format.extension()));
    let _cleanup = TempFileCleanup::new(vec![wav_path.clone(), transcript_path.clone()]);

    fs::write(&wav_path, wav_data).map_err(|e| {
        format!(
            "Failed to write temporary audio file ({}): {}",
            wav_path.display(),
            e
        )
    })?;

    let mut whisper_args = vec![
        "-m".to_string(),
        model_path.to_string_lossy().to_string(),
        "-f".to_string(),
        wav_path.to_string_lossy().to_string(),
        format.cli_flag().to_string(),
        "-of".to_string(),
        output_base.to_string_lossy().to_string(),
    ];

    if language != "auto" {
        whisper_args.push("-l".to_string());
        whisper_args.push(language.to_string());
    }

    if diarization_mode == "tdrz_2speaker" {
        whisper_args.push("-tdrz".to_string());
    }

    let whisper_output = run_whisper(app, &whisper_args).await?;
    if !whisper_output.success {
        return Err(format!(
            "Whisper failed: {}",
            process_output_detail(&whisper_output.stdout, &whisper_output.stderr)
        ));
    }

    let content = fs::read_to_string(&transcript_path).map_err(|e| {
        format!(
            "Whisper ran but transcript file could not be read ({}): {}",
            transcript_path.display(),
            e
        )
    })?;

    Ok(WhisperTranscriptOutput {
        content,
        used_sidecar: whisper_output.used_sidecar,
    })
}

fn parse_srt_timestamp(raw: &str) -> Option<u64> {
    let cleaned = raw.trim().replace(',', ".");
    let parts = cleaned.split(':').collect::<Vec<&str>>();
    if parts.len() != 3 {
        return None;
    }

    let hours = parts[0].parse::<u64>().ok()?;
    let minutes = parts[1].parse::<u64>().ok()?;
    let sec_parts = parts[2].split('.').collect::<Vec<&str>>();
    if sec_parts.len() != 2 {
        return None;
    }

    let seconds = sec_parts[0].parse::<u64>().ok()?;
    let millis = match sec_parts[1].len() {
        0 => 0,
        1 => sec_parts[1].parse::<u64>().ok()? * 100,
        2 => sec_parts[1].parse::<u64>().ok()? * 10,
        _ => sec_parts[1].chars().take(3).collect::<String>().parse::<u64>().ok()?,
    };

    Some((((hours * 60) + minutes) * 60 + seconds) * 1000 + millis)
}

fn parse_srt_segments(text: &str, speaker: &str) -> Vec<TimestampedSegment> {
    let normalized = text.replace("\r\n", "\n");
    let mut segments = Vec::new();

    for block in normalized.split("\n\n") {
        let lines = block
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<&str>>();

        if lines.len() < 2 {
            continue;
        }

        let time_index = if lines[0].chars().all(|char| char.is_ascii_digit()) {
            1
        } else {
            0
        };

        if lines.len() <= time_index {
            continue;
        }

        let Some((start_raw, end_raw)) = lines[time_index].split_once("-->") else {
            continue;
        };

        let Some(start_ms) = parse_srt_timestamp(start_raw) else {
            continue;
        };
        let Some(end_ms) = parse_srt_timestamp(end_raw) else {
            continue;
        };

        let body = sanitize_transcript_text(
            &lines
            .iter()
            .skip(time_index + 1)
            .copied()
            .collect::<Vec<&str>>()
            .join(" ")
            .trim()
            .to_string(),
        );

        if body.is_empty() {
            continue;
        }

        segments.push(TimestampedSegment {
            speaker: speaker.to_string(),
            start_ms,
            end_ms,
            text: body,
        });
    }

    if segments.is_empty() {
        let fallback = normalize_transcript(text);
        if !fallback.is_empty() {
            segments.push(TimestampedSegment {
                speaker: speaker.to_string(),
                start_ms: 0,
                end_ms: 0,
                text: fallback,
            });
        }
    }

    segments
}

fn coalesce_channel_segments(
    segments: Vec<TimestampedSegment>,
    max_gap_ms: u64,
) -> Vec<TimestampedSegment> {
    let mut merged: Vec<TimestampedSegment> = Vec::new();

    for segment in segments {
        if let Some(last) = merged.last_mut() {
            if last.speaker == segment.speaker
                && segment.start_ms >= last.start_ms
                && segment.start_ms.saturating_sub(last.end_ms) <= max_gap_ms
            {
                last.end_ms = last.end_ms.max(segment.end_ms);
                last.text = format!("{} {}", last.text, segment.text).trim().to_string();
                continue;
            }
        }

        merged.push(segment);
    }

    merged
}

fn shift_segments(segments: &mut [TimestampedSegment], offset_ms: u64) {
    if offset_ms == 0 {
        return;
    }

    for segment in segments {
        segment.start_ms = segment.start_ms.saturating_add(offset_ms);
        segment.end_ms = segment.end_ms.saturating_add(offset_ms);
    }
}

fn extract_word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let mut token_start: Option<usize> = None;

    for (index, ch) in text.char_indices() {
        let is_word = ch.is_ascii_alphanumeric() || ch == '\'';
        match (token_start, is_word) {
            (None, true) => token_start = Some(index),
            (Some(start), false) => {
                let token = &text[start..index];
                let normalized = token
                    .chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '\'')
                    .collect::<String>()
                    .to_ascii_lowercase();

                if !normalized.is_empty() {
                    spans.push(WordSpan {
                        start,
                        end: index,
                        normalized,
                    });
                }
                token_start = None;
            }
            _ => {}
        }
    }

    if let Some(start) = token_start {
        let token = &text[start..];
        let normalized = token
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '\'')
            .collect::<String>()
            .to_ascii_lowercase();

        if !normalized.is_empty() {
            spans.push(WordSpan {
                start,
                end: text.len(),
                normalized,
            });
        }
    }

    spans
}

fn collect_word_tokens(text: &str) -> Vec<String> {
    extract_word_spans(text)
        .into_iter()
        .map(|span| span.normalized)
        .collect()
}

fn collapse_spacing(text: &str) -> String {
    let mut output = String::new();
    let mut previous_was_space = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !previous_was_space {
                output.push(' ');
                previous_was_space = true;
            }
            continue;
        }

        if matches!(ch, '.' | ',' | '!' | '?' | ';' | ':') && output.ends_with(' ') {
            output.pop();
        }

        output.push(ch);
        previous_was_space = false;
    }

    output.trim().to_string()
}

fn longest_duplicate_range(
    spans: &[WordSpan],
    reference_tokens: &[String],
    min_match_tokens: usize,
) -> Option<(usize, usize)> {
    let mut best_range: Option<(usize, usize)> = None;
    let mut best_len = 0usize;

    for start_index in 0..spans.len() {
        for reference_start in 0..reference_tokens.len() {
            if spans[start_index].normalized != reference_tokens[reference_start] {
                continue;
            }

            let mut match_len = 0usize;
            while start_index + match_len < spans.len()
                && reference_start + match_len < reference_tokens.len()
                && spans[start_index + match_len].normalized
                    == reference_tokens[reference_start + match_len]
            {
                match_len += 1;
            }

            if match_len >= min_match_tokens && match_len > best_len {
                best_len = match_len;
                let start_byte = spans[start_index].start;
                let end_byte = spans[start_index + match_len - 1].end;
                best_range = Some((start_byte, end_byte));
            }
        }
    }

    best_range
}

fn segments_overlap_with_margin(
    left_start: u64,
    left_end: u64,
    right_start: u64,
    right_end: u64,
    margin_ms: u64,
) -> bool {
    let left_start = left_start.saturating_sub(margin_ms);
    let left_end = left_end.saturating_add(margin_ms);
    let right_start = right_start.saturating_sub(margin_ms);
    let right_end = right_end.saturating_add(margin_ms);

    left_start <= right_end && right_start <= left_end
}

fn strip_reference_leakage_from_segment(
    text: &str,
    reference_segments: &[TimestampedSegment],
    start_ms: u64,
    end_ms: u64,
) -> String {
    let overlapping_reference = reference_segments
        .iter()
        .filter(|segment| {
            segments_overlap_with_margin(start_ms, end_ms, segment.start_ms, segment.end_ms, 1200)
        })
        .map(|segment| segment.text.as_str())
        .collect::<Vec<&str>>();

    if overlapping_reference.is_empty() {
        return text.trim().to_string();
    }

    let reference_tokens = collect_word_tokens(&overlapping_reference.join(" "));
    if reference_tokens.len() < 4 {
        return text.trim().to_string();
    }

    let mut cleaned = text.trim().to_string();
    loop {
        let spans = extract_word_spans(&cleaned);
        if spans.len() < 4 {
            break;
        }

        let Some((start_byte, end_byte)) = longest_duplicate_range(&spans, &reference_tokens, 4)
        else {
            break;
        };

        cleaned.replace_range(start_byte..end_byte, " ");
        cleaned = collapse_spacing(&cleaned);
    }

    cleaned
}

fn merge_source_segments(
    microphone_segments: Vec<TimestampedSegment>,
    system_segments: Vec<TimestampedSegment>,
) -> String {
    let mut segments = Vec::new();

    for mut segment in microphone_segments {
        let cleaned = strip_reference_leakage_from_segment(
            &segment.text,
            &system_segments,
            segment.start_ms,
            segment.end_ms,
        );

        if extract_word_spans(&cleaned).is_empty() {
            continue;
        }

        segment.text = cleaned;
        segments.push(segment);
    }

    segments.extend(system_segments);
    segments.sort_by(|left, right| {
        left.start_ms
            .cmp(&right.start_ms)
            .then_with(|| left.end_ms.cmp(&right.end_ms))
            .then_with(|| left.speaker.cmp(&right.speaker))
    });

    let mut merged: Vec<TimestampedSegment> = Vec::new();
    for segment in segments {
        if let Some(last) = merged.last_mut() {
            let same_speaker = last.speaker == segment.speaker;
            let gap_ms = segment.start_ms.saturating_sub(last.end_ms);
            if same_speaker && gap_ms <= 500 {
                last.end_ms = last.end_ms.max(segment.end_ms);
                last.text = format!("{} {}", last.text, segment.text).trim().to_string();
                continue;
            }
        }

        merged.push(segment);
    }

    merged
        .into_iter()
        .map(|segment| format!("{}: {}", segment.speaker, segment.text))
        .collect::<Vec<String>>()
        .join("\n\n")
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() {
                char.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();

    sanitized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join("-")
}

fn save_raw_audio_copies(
    settings: &AppSettings,
    base_name: &str,
    coachnotes_client: Option<&str>,
    primary_audio: &[u8],
    microphone_audio: &[u8],
    system_audio: &[u8],
) -> Result<Vec<String>, String> {
    let audio_dir = resolve_transcript_dir(settings);
    fs::create_dir_all(&audio_dir).map_err(|e| {
        format!(
            "Failed to create transcript directory ({}): {}",
            audio_dir.display(),
            e
        )
    })?;

    let client_suffix = coachnotes_client
        .map(sanitize_filename_component)
        .filter(|value| !value.is_empty())
        .map(|value| format!("-{}", value))
        .unwrap_or_default();

    let stem = format!("{}{}", base_name, client_suffix);
    let mut saved_paths = Vec::new();

    let primary_path = audio_dir.join(format!("{}-recording.wav", stem));
    fs::write(&primary_path, primary_audio).map_err(|e| {
        format!(
            "Failed to write raw audio file ({}): {}",
            primary_path.display(),
            e
        )
    })?;
    saved_paths.push(primary_path.to_string_lossy().to_string());

    if !microphone_audio.is_empty() && !system_audio.is_empty() {
        let microphone_path = audio_dir.join(format!("{}-coach-mic.wav", stem));
        fs::write(&microphone_path, microphone_audio).map_err(|e| {
            format!(
                "Failed to write microphone audio file ({}): {}",
                microphone_path.display(),
                e
            )
        })?;
        saved_paths.push(microphone_path.to_string_lossy().to_string());

        let system_path = audio_dir.join(format!("{}-client-system.wav", stem));
        fs::write(&system_path, system_audio).map_err(|e| {
            format!(
                "Failed to write system audio file ({}): {}",
                system_path.display(),
                e
            )
        })?;
        saved_paths.push(system_path.to_string_lossy().to_string());
    }

    Ok(saved_paths)
}

fn estimate_duration_seconds(wav_data: &[u8]) -> u64 {
    if wav_data.len() <= 44 {
        return 0;
    }

    let sample_bytes = wav_data.len().saturating_sub(44);
    let samples = sample_bytes / 2;
    (samples / 16_000) as u64
}

fn yaml_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

fn normalize_transcript(text: &str) -> String {
    sanitize_transcript_text(
        &text.lines()
        .map(str::trim)
        .collect::<Vec<&str>>()
        .join("\n")
        .trim()
        .to_string(),
    )
}

fn sanitize_transcript_text(text: &str) -> String {
    text.replace(BLANK_AUDIO_MARKER, "")
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
        .trim()
        .to_string()
}

fn apply_tdrz_speaker_labels(text: &str) -> (String, bool) {
    if !text.contains(SPEAKER_TURN_MARKER) {
        return (normalize_transcript(text), false);
    }

    let mut speaker_a_turn = true;
    let mut segments = Vec::new();

    for block in text.split(SPEAKER_TURN_MARKER) {
        let cleaned = block
            .split_whitespace()
            .collect::<Vec<&str>>()
            .join(" ")
            .trim()
            .to_string();

        if cleaned.is_empty() {
            continue;
        }

        let speaker = if speaker_a_turn {
            "Speaker A"
        } else {
            "Speaker B"
        };
        segments.push(format!("{}: {}", speaker, cleaned));
        speaker_a_turn = !speaker_a_turn;
    }

    if segments.is_empty() {
        return (normalize_transcript(text), false);
    }

    (segments.join("\n\n"), true)
}

fn build_markdown_transcript(
    transcript: &str,
    coachnotes_client: Option<&str>,
    model: &str,
    language: &str,
    diarization_mode: &str,
    created_at: &str,
    date: &str,
    duration_seconds: u64,
    coachnotes_metadata: bool,
    speaker_labels: Option<(&str, &str)>,
) -> String {
    let client_value = coachnotes_client.unwrap_or("");

    if coachnotes_metadata {
        let (speaker_1, speaker_2) = speaker_labels.unwrap_or(("Coach", "Client"));

        return format!(
            "---\nclient: {}\ndate: {}\ntitle: {}\nnote_type: {}\nsource: {}\ntranscript: true\nspeakers:\n  - {}\n  - {}\ntags:\n  - {}\n  - {}\nsource_app: {}\ncreated_at: {}\nmodel: {}\nlanguage: {}\ndiarization_mode: {}\nduration_seconds: {}\n---\n# Transcript\n\n{}\n",
            yaml_quote(client_value),
            yaml_quote(date),
            yaml_quote("Session Transcript"),
            yaml_quote("transcript"),
            yaml_quote("coachnotes-voice-app"),
            yaml_quote(speaker_1),
            yaml_quote(speaker_2),
            yaml_quote("transcript"),
            yaml_quote("coaching-session"),
            yaml_quote("Echo Scribe"),
            yaml_quote(created_at),
            yaml_quote(model),
            yaml_quote(language),
            yaml_quote(diarization_mode),
            duration_seconds,
            transcript
        );
    }

    format!(
        "---\ntitle: {}\ndate: {}\nsource_app: {}\ncreated_at: {}\nmodel: {}\nlanguage: {}\ndiarization_mode: {}\nduration_seconds: {}\n---\n# Transcript\n\n{}\n",
        yaml_quote("Session Transcript"),
        yaml_quote(date),
        yaml_quote("Echo Scribe"),
        yaml_quote(created_at),
        yaml_quote(model),
        yaml_quote(language),
        yaml_quote(diarization_mode),
        duration_seconds,
        transcript
    )
}

fn build_setup_state(app: &AppHandle) -> Result<SetupState, String> {
    let settings = load_settings(app)?;
    let models_directory = models_dir(app)?;
    let transcript_directory = resolve_transcript_dir(&settings);

    let models = MODEL_CATALOG
        .iter()
        .map(|entry| {
            let path = models_directory.join(format!("ggml-{}.bin", entry.id));
            ModelState {
                id: entry.id.to_string(),
                label: entry.label.to_string(),
                size_mb: entry.size_mb,
                downloaded: path.exists(),
                path: path.to_string_lossy().to_string(),
            }
        })
        .collect::<Vec<ModelState>>();

    let selected_model_downloaded = models
        .iter()
        .find(|entry| entry.id == settings.selected_model)
        .map(|entry| entry.downloaded)
        .unwrap_or(false);

    let sidecar_ready = is_sidecar_available();
    let runtime_ready = if cfg!(debug_assertions) {
        true
    } else {
        sidecar_ready
    };

    let coachnotes_root_dir = sanitize_non_empty(settings.coachnotes_root_dir.clone());
    let coachnotes_clients = if let Some(root) = &coachnotes_root_dir {
        list_coachnotes_clients_from_root(Path::new(root)).unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(SetupState {
        selected_model: settings.selected_model,
        transcript_dir: transcript_directory.to_string_lossy().to_string(),
        transcript_format: "md".to_string(),
        models_dir: models_directory.to_string_lossy().to_string(),
        models,
        ready: selected_model_downloaded && runtime_ready,
        sidecar_ready,
        coachnotes_enabled: settings.coachnotes_enabled,
        coachnotes_root_dir,
        coachnotes_clients,
        coachnotes_client: sanitize_non_empty(settings.coachnotes_client),
        diarization_mode: settings.diarization_mode,
        diarization_capabilities: DiarizationCapabilities {
            tdrz_english_only: true,
        },
    })
}

#[tauri::command]
async fn get_setup_state(app: AppHandle) -> Result<SetupState, String> {
    build_setup_state(&app)
}

#[tauri::command]
async fn set_selected_model(app: AppHandle, model: String) -> Result<SetupState, String> {
    validate_model(&model)?;

    let mut settings = load_settings(&app)?;
    settings.selected_model = model;
    save_settings(&app, &settings)?;

    build_setup_state(&app)
}

#[tauri::command]
async fn set_transcript_directory(app: AppHandle, directory: String) -> Result<SetupState, String> {
    let directory = directory.trim();
    if directory.is_empty() {
        return Err("Directory path cannot be empty.".to_string());
    }

    let directory_path = PathBuf::from(directory);
    fs::create_dir_all(&directory_path).map_err(|e| {
        format!(
            "Could not create transcript directory ({}): {}",
            directory_path.display(),
            e
        )
    })?;

    let mut settings = load_settings(&app)?;
    settings.transcript_dir = Some(directory_path.to_string_lossy().to_string());
    save_settings(&app, &settings)?;

    build_setup_state(&app)
}

#[tauri::command]
async fn set_diarization_mode(app: AppHandle, mode: String) -> Result<SetupState, String> {
    let mut settings = load_settings(&app)?;
    settings.diarization_mode = validate_diarization_mode(&mode).to_string();
    settings.diarization_mode_configured = true;
    save_settings(&app, &settings)?;

    build_setup_state(&app)
}

#[tauri::command]
async fn get_coachnotes_clients(root_dir: String) -> Result<Vec<String>, String> {
    let trimmed = root_dir.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    list_coachnotes_clients_from_root(Path::new(trimmed))
}

#[tauri::command]
async fn set_coachnotes_settings(
    app: AppHandle,
    input: CoachNotesSettingsInput,
) -> Result<SetupState, String> {
    let mut settings = load_settings(&app)?;

    let root = sanitize_non_empty(input.root_dir);
    if let Some(root_dir) = &root {
        fs::create_dir_all(root_dir).map_err(|e| {
            format!(
                "Failed to ensure CoachNotes root exists ({}): {}",
                root_dir, e
            )
        })?;
    }

    settings.coachnotes_enabled = input.enabled;
    settings.coachnotes_root_dir = root;
    settings.coachnotes_client = sanitize_non_empty(input.client);
    save_settings(&app, &settings)?;

    build_setup_state(&app)
}

#[tauri::command]
async fn download_model(
    app: AppHandle,
    options: ModelDownloadOptions,
) -> Result<ModelDownloadResult, String> {
    let model = validate_model(&options.model)?;

    let model_dir = models_dir(&app)?;
    fs::create_dir_all(&model_dir).map_err(|e| {
        format!(
            "Failed to create models directory ({}): {}",
            model_dir.display(),
            e
        )
    })?;

    let target_path = model_dir.join(format!("ggml-{}.bin", model.id));
    let temp_path = target_path.with_extension("bin.part");

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {}", e))?;
    let expected_checksum = model.sha256;

    if target_path.exists() {
        emit_model_download_progress(&app, model.id, 1, 0, None, "Verifying existing model...");
        let existing_checksum = sha256_for_file(&target_path).await?;
        if existing_checksum == expected_checksum {
            emit_model_download_progress(&app, model.id, 100, 0, None, "Model already downloaded.");
            return Ok(ModelDownloadResult {
                model: model.id.to_string(),
                path: target_path.to_string_lossy().to_string(),
            });
        }
        let _ = fs::remove_file(&target_path);
    }

    let _ = fs::remove_file(&temp_path);
    emit_model_download_progress(&app, model.id, 2, 0, None, "Starting download...");

    let response = client
        .get(model.url)
        .send()
        .await
        .map_err(|e| format!("Model download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Model download failed with HTTP status {}",
            response.status()
        ));
    }

    let total_bytes = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&temp_path)
        .await
        .map_err(|e| format!("Failed to create temp model file: {}", e))?;

    let mut hasher = Sha256::new();
    let mut downloaded_bytes: u64 = 0;

    while let Some(next) = stream.next().await {
        let chunk = next.map_err(|e| format!("Download stream failed: {}", e))?;

        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Failed to write model file: {}", e))?;

        hasher.update(&chunk);
        downloaded_bytes += chunk.len() as u64;

        let percent = total_bytes
            .map(|total| ((downloaded_bytes.saturating_mul(100)) / total.max(1)) as u32)
            .unwrap_or(0)
            .min(99);

        emit_model_download_progress(
            &app,
            model.id,
            percent.max(2),
            downloaded_bytes,
            total_bytes,
            "Downloading model...",
        );
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush model file: {}", e))?;

    let actual_checksum = format!("{:x}", hasher.finalize());

    if actual_checksum != expected_checksum {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Checksum mismatch for {} model. Expected {}, got {}.",
            model.id, expected_checksum, actual_checksum
        ));
    }

    if target_path.exists() {
        let _ = fs::remove_file(&target_path);
    }

    tokio::fs::rename(&temp_path, &target_path)
        .await
        .map_err(|e| format!("Failed to finalize model file: {}", e))?;

    emit_model_download_progress(
        &app,
        model.id,
        100,
        downloaded_bytes,
        total_bytes,
        "Model download complete.",
    );

    Ok(ModelDownloadResult {
        model: model.id.to_string(),
        path: target_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
async fn start_system_audio_recording(
    app: AppHandle,
    state: State<'_, SystemAudioCaptureState>,
) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        return Err("System audio capture is only supported on macOS.".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let mut guard = state
            .session
            .lock()
            .map_err(|_| "Failed to lock system audio state.".to_string())?;

        if guard.is_some() {
            return Err("System audio capture is already running.".to_string());
        }

        let timestamp = unix_timestamp_secs()?;
        let temp_dir = std::env::temp_dir().join("echo-scribe");
        fs::create_dir_all(&temp_dir)
            .map_err(|e| format!("Failed to create temporary directory: {}", e))?;
        let output_path = temp_dir.join(format!("system-audio-{}.wav", timestamp));

        let sidecar_path = resolve_system_audio_capture_sidecar_path(&app)?;
        let mut command = StdCommand::new(&sidecar_path);
        command
            .arg("record")
            .arg("--output")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command.spawn().map_err(|e| {
            format!(
                "Failed to start system audio capture binary ({}): {}",
                sidecar_path.display(),
                e
            )
        })?;

        *guard = Some(SystemAudioCaptureSession { child, output_path });
        Ok(())
    }
}

#[tauri::command]
async fn stop_system_audio_recording(
    state: State<'_, SystemAudioCaptureState>,
) -> Result<SystemAudioCaptureResult, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        return Err("System audio capture is only supported on macOS.".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let mut session = {
            let mut guard = state
                .session
                .lock()
                .map_err(|_| "Failed to lock system audio state.".to_string())?;

            guard
                .take()
                .ok_or_else(|| "System audio capture is not currently running.".to_string())?
        };
        let _cleanup = TempFileCleanup::new(vec![session.output_path.clone()]);

        let _ = session.child.stdin.take();

        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = session
                .child
                .try_wait()
                .map_err(|e| format!("Failed to wait for system audio capture process: {}", e))?
            {
                break status;
            }

            if Instant::now() >= deadline {
                let _ = session.child.kill();
                let _ = session.child.wait();
                return Err(
                    "Timed out while stopping system audio capture. Please try again.".to_string(),
                );
            }

            std::thread::sleep(Duration::from_millis(50));
        };

        let mut stdout = Vec::new();
        if let Some(mut stdout_reader) = session.child.stdout.take() {
            let _ = stdout_reader.read_to_end(&mut stdout);
        }

        let mut stderr = Vec::new();
        if let Some(mut stderr_reader) = session.child.stderr.take() {
            let _ = stderr_reader.read_to_end(&mut stderr);
        }

        if !status.success() {
            let stderr_text = String::from_utf8_lossy(&stderr).trim().to_string();
            if stderr_text.is_empty() {
                return Err("System audio capture exited with an error.".to_string());
            }
            return Err(format!("System audio capture failed: {}", stderr_text));
        }

        let wav_data = fs::read(&session.output_path).map_err(|e| {
            format!(
                "Failed to read captured system audio file ({}): {}",
                session.output_path.display(),
                e
            )
        })?;

        if wav_data.len() <= 44 {
            return Err(
                "No shared audio was captured. Start playback first, then record again."
                    .to_string(),
            );
        }

        let metadata = if stdout.is_empty() {
            SystemAudioCaptureMetadata {
                first_audio_wall_time_ms: 0,
            }
        } else {
            serde_json::from_slice::<SystemAudioCaptureMetadata>(&stdout).map_err(|error| {
                format!(
                    "System audio capture returned invalid metadata: {}",
                    error
                )
            })?
        };

        Ok(SystemAudioCaptureResult {
            audio_data: wav_data,
            first_audio_wall_time_ms: metadata.first_audio_wall_time_ms,
        })
    }
}

#[tauri::command]
async fn transcribe_recording(
    app: AppHandle,
    options: TranscriptionOptions,
) -> Result<TranscriptionResult, String> {
    let primary_audio = if !options.audio_data.is_empty() {
        options.audio_data.as_slice()
    } else if !options.system_audio_data.is_empty() {
        options.system_audio_data.as_slice()
    } else if !options.microphone_audio_data.is_empty() {
        options.microphone_audio_data.as_slice()
    } else {
        return Err("No audio data provided. Record audio first.".to_string());
    };

    validate_model(&options.model)?;
    let model_path = model_file_path(&app, &options.model)?;

    if !model_path.exists() {
        return Err(format!(
            "Model '{}' is not downloaded yet. Use Setup to download it first.",
            options.model
        ));
    }

    let mut warnings = Vec::new();
    let settings = load_settings(&app)?;
    let mut speaker_mode_used = if options.diarization_mode.trim().is_empty() {
        validate_diarization_mode(&settings.diarization_mode).to_string()
    } else {
        validate_diarization_mode(&options.diarization_mode).to_string()
    };

    let has_dual_source_audio =
        !options.microphone_audio_data.is_empty() && !options.system_audio_data.is_empty();

    if speaker_mode_used == "source_aware_2speaker" && !has_dual_source_audio {
        warnings.push(
            "Two-speaker source-aware mode requires both microphone and system audio capture. Falling back to standard transcription."
                .to_string(),
        );
        speaker_mode_used = "none".to_string();
    }

    if speaker_mode_used == "tdrz_2speaker" {
        if options.language != "en" {
            warnings.push(
                "Whisper diarization fallback is English-only. Falling back to standard transcription."
                    .to_string(),
            );
            speaker_mode_used = "none".to_string();
        } else if options.model != "small.en-tdrz" {
            warnings.push(
                "Whisper diarization fallback requires the small.en-tdrz model. Falling back to standard transcription."
                    .to_string(),
            );
            speaker_mode_used = "none".to_string();
        }
    }

    let timestamp = unix_timestamp_secs()?;
    let mut diarization_applied = false;
    let transcript = if speaker_mode_used == "source_aware_2speaker" {
        emit_progress(&app, 5, "Preparing separate speaker channels...");

        let microphone_output = transcribe_with_temp_output(
            &app,
            &model_path,
            &options.microphone_audio_data,
            &options.language,
            "none",
            WhisperFileFormat::Srt,
            &format!("recording-{}-coach-mic", timestamp),
        )
        .await?;

        emit_progress(&app, 50, "Transcribing client system audio...");

        let system_output = transcribe_with_temp_output(
            &app,
            &model_path,
            &options.system_audio_data,
            &options.language,
            "none",
            WhisperFileFormat::Srt,
            &format!("recording-{}-client-system", timestamp),
        )
        .await?;

        if !microphone_output.used_sidecar || !system_output.used_sidecar {
            warnings.push(
                "Using local whisper binary fallback in debug mode. Release builds use sidecar."
                    .to_string(),
            );
        }

        let microphone_segments = parse_srt_segments(&microphone_output.content, "Coach");
        let mut system_segments =
            coalesce_channel_segments(parse_srt_segments(&system_output.content, "Client"), 750);
        shift_segments(&mut system_segments, options.system_audio_offset_ms);

        if microphone_segments.is_empty() {
            warnings.push(
                "Microphone channel did not produce timestamped transcript segments.".to_string(),
            );
        }
        if system_segments.is_empty() {
            warnings.push(
                "System audio channel did not produce timestamped transcript segments."
                    .to_string(),
            );
        }

        emit_progress(&app, 85, "Merging separate speaker transcripts...");
        diarization_applied = true;
        merge_source_segments(microphone_segments, system_segments)
    } else {
        emit_progress(
            &app,
            5,
            if speaker_mode_used == "tdrz_2speaker" {
                "Preparing diarization fallback..."
            } else {
                "Preparing recording..."
            },
        );

        let transcript_output = transcribe_with_temp_output(
            &app,
            &model_path,
            primary_audio,
            &options.language,
            &speaker_mode_used,
            WhisperFileFormat::Txt,
            &format!("recording-{}", timestamp),
        )
        .await?;

        if !transcript_output.used_sidecar {
            warnings.push(
                "Using local whisper binary fallback in debug mode. Release builds use sidecar."
                    .to_string(),
            );
        }

        emit_progress(&app, 85, "Reading transcript...");

        if speaker_mode_used == "tdrz_2speaker" {
            let (formatted, applied) = apply_tdrz_speaker_labels(&transcript_output.content);
            if !applied {
                warnings.push(
                    "Whisper diarization fallback did not produce speaker boundaries because whisper.cpp returned no [SPEAKER_TURN] markers. Output is unsegmented. This is common when voices are too similar/overlapped or only one voice is dominant; try clearer turn-taking, louder remote audio, or use source-aware mode with separate system + microphone capture."
                        .to_string(),
                );
            }
            diarization_applied = applied;
            formatted
        } else {
            normalize_transcript(&transcript_output.content)
        }
    };

    if transcript.is_empty() {
        return Err("Whisper returned an empty transcript.".to_string());
    }

    let output_mode = validate_output_mode(&options.output_mode);
    let mut save_destination: Option<PathBuf> = None;
    let now = now_local_or_utc();
    let date = format_date(now);
    let time_compact = format_time_compact(now);
    let created_at = format_iso8601(now);
    let coachnotes_metadata = output_mode == "coachnotes" && settings.coachnotes_enabled;
    let frontmatter_client = if coachnotes_metadata {
        sanitize_non_empty(options.client.clone())
            .or_else(|| sanitize_non_empty(settings.coachnotes_client.clone()))
    } else {
        None
    };

    if options.save_markdown {
        if output_mode == "coachnotes" && settings.coachnotes_enabled {
            let root = sanitize_non_empty(settings.coachnotes_root_dir.clone());
            let selected_client = sanitize_non_empty(options.client.clone())
                .or_else(|| sanitize_non_empty(settings.coachnotes_client.clone()));

            match (root, selected_client) {
                (Some(root), Some(client)) => {
                    let client_dir = PathBuf::from(root).join(&client);
                    fs::create_dir_all(&client_dir).map_err(|e| {
                        format!(
                            "Failed to create CoachNotes client directory ({}): {}",
                            client_dir.display(),
                            e
                        )
                    })?;
                    let path = client_dir.join(format!("{}-transcript-{}.md", date, time_compact));
                    save_destination = Some(path);
                }
                _ => {
                    warnings.push(
                        "CoachNotes mode is enabled but root/client is incomplete. Saving to standard transcript folder instead."
                            .to_string(),
                    );
                }
            }
        }

        if save_destination.is_none() {
            let transcript_dir = resolve_transcript_dir(&settings);
            fs::create_dir_all(&transcript_dir).map_err(|e| {
                format!(
                    "Failed to create transcript directory ({}): {}",
                    transcript_dir.display(),
                    e
                )
            })?;

            save_destination = Some(transcript_dir.join(format!("transcript-{}.md", timestamp)));
        }
    }

    let duration_seconds = [
        estimate_duration_seconds(primary_audio),
        estimate_duration_seconds(&options.microphone_audio_data),
        estimate_duration_seconds(&options.system_audio_data),
    ]
    .into_iter()
    .max()
    .unwrap_or(0);

    let markdown = build_markdown_transcript(
        &transcript,
        frontmatter_client.as_deref(),
        &options.model,
        &options.language,
        &speaker_mode_used,
        &created_at,
        &date,
        duration_seconds,
        coachnotes_metadata,
        if speaker_mode_used == "source_aware_2speaker" {
            Some(("Coach", "Client"))
        } else if speaker_mode_used == "tdrz_2speaker" && diarization_applied {
            Some(("Speaker A", "Speaker B"))
        } else {
            Some(("Coach", "Client"))
        },
    );

    let saved_path = if let Some(path) = save_destination {
        fs::write(&path, markdown).map_err(|e| {
            format!(
                "Failed to write transcript file ({}): {}",
                path.display(),
                e
            )
        })?;

        Some(path.to_string_lossy().to_string())
    } else {
        None
    };

    let saved_audio_paths = if options.save_raw_audio {
        save_raw_audio_copies(
            &settings,
            &format!("{}-transcript-{}", date, time_compact),
            frontmatter_client.as_deref(),
            primary_audio,
            &options.microphone_audio_data,
            &options.system_audio_data,
        )?
    } else {
        Vec::new()
    };

    emit_progress(&app, 100, "Transcription complete!");

    Ok(TranscriptionResult {
        transcript,
        saved_path,
        saved_audio_paths,
        format: "md".to_string(),
        diarization_applied,
        speaker_mode_used,
        warnings,
    })
}

#[tauri::command]
async fn show_in_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        StdCommand::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "windows")]
    {
        StdCommand::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        StdCommand::new("xdg-open")
            .arg(Path::new(&path).parent().unwrap_or_else(|| Path::new(".")))
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(SystemAudioCaptureState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_setup_state,
            set_selected_model,
            set_transcript_directory,
            set_diarization_mode,
            get_coachnotes_clients,
            set_coachnotes_settings,
            download_model,
            start_system_audio_recording,
            stop_system_audio_recording,
            transcribe_recording,
            show_in_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
