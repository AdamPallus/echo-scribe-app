use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;
use time::{format_description::well_known::Rfc3339, macros::format_description, OffsetDateTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COACHNOTES_DELETED_DIR: &str = "Deleted Notes";
const SPEAKER_TURN_MARKER: &str = "[SPEAKER_TURN]";

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
            diarization_mode: "none".to_string(),
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
    model: String,
    language: String,
    save_markdown: bool,
    output_mode: String,
    client: Option<String>,
    diarization_mode: String,
}

#[derive(Debug, Serialize)]
pub struct TranscriptionResult {
    transcript: String,
    saved_path: Option<String>,
    format: String,
    diarization_applied: bool,
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
    stderr: Vec<u8>,
    used_sidecar: bool,
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
        "tdrz_2speaker" => "tdrz_2speaker",
        _ => "none",
    }
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

fn get_whisper_path() -> PathBuf {
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

fn sidecar_binary_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    Some(parent.join("whisper-cli"))
}

fn is_sidecar_available() -> bool {
    sidecar_binary_path().map(|p| p.exists()).unwrap_or(false)
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
            stderr: output.stderr,
            used_sidecar: true,
        });
    }

    #[cfg(debug_assertions)]
    {
        if let Ok(command) = app.shell().sidecar("whisper-cli") {
            if let Ok(output) = command.args(args).output().await {
                return Ok(WhisperOutput {
                    success: output.status.success(),
                    stderr: output.stderr,
                    used_sidecar: true,
                });
            }
        }

        let whisper_path = get_whisper_path();
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

        Ok(WhisperOutput {
            success: fallback.status.success(),
            stderr: fallback.stderr,
            used_sidecar: false,
        })
    }
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
    text.lines()
        .map(str::trim)
        .collect::<Vec<&str>>()
        .join("\n")
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
    client: Option<&str>,
    model: &str,
    language: &str,
    diarization_mode: &str,
    created_at: &str,
    date: &str,
    duration_seconds: u64,
) -> String {
    let client_value = client.unwrap_or("");

    format!(
        "---\ntitle: {}\ndate: {}\nclient: {}\nsource_app: {}\ncreated_at: {}\nmodel: {}\nlanguage: {}\ndiarization_mode: {}\nduration_seconds: {}\n---\n# Transcript\n\n{}\n",
        yaml_quote("Session Transcript"),
        yaml_quote(date),
        yaml_quote(client_value),
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
async fn transcribe_recording(
    app: AppHandle,
    options: TranscriptionOptions,
) -> Result<TranscriptionResult, String> {
    if options.audio_data.is_empty() {
        return Err("No audio data provided. Record audio first.".to_string());
    }

    validate_model(&options.model)?;
    let model_path = model_file_path(&app, &options.model)?;

    if !model_path.exists() {
        return Err(format!(
            "Model '{}' is not downloaded yet. Use Setup to download it first.",
            options.model
        ));
    }

    let mut warnings = Vec::new();

    let mut diarization_mode = validate_diarization_mode(&options.diarization_mode).to_string();
    if diarization_mode == "none" {
        let settings = load_settings(&app)?;
        diarization_mode = validate_diarization_mode(&settings.diarization_mode).to_string();
    }

    if diarization_mode == "tdrz_2speaker" {
        if options.language != "en" {
            warnings.push(
                "2-speaker mode is English-only. Falling back to standard transcription."
                    .to_string(),
            );
            diarization_mode = "none".to_string();
        } else if options.model != "small.en-tdrz" {
            warnings.push(
                "2-speaker mode requires the small.en-tdrz model. Falling back to standard transcription."
                    .to_string(),
            );
            diarization_mode = "none".to_string();
        }
    }

    let timestamp = unix_timestamp_secs()?;
    let temp_dir = std::env::temp_dir().join("echo-scribe");
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Failed to create temporary directory: {}", e))?;

    let wav_path = temp_dir.join(format!("recording-{}.wav", timestamp));
    let output_base = temp_dir.join(format!("recording-{}", timestamp));
    let txt_temp_path = temp_dir.join(format!("recording-{}.txt", timestamp));

    emit_progress(&app, 5, "Preparing recording...");
    fs::write(&wav_path, &options.audio_data)
        .map_err(|e| format!("Failed to write temporary audio file: {}", e))?;

    emit_progress(
        &app,
        20,
        &format!("Transcribing with {} model...", options.model),
    );

    let mut whisper_args = vec![
        "-m".to_string(),
        model_path.to_string_lossy().to_string(),
        "-f".to_string(),
        wav_path.to_string_lossy().to_string(),
        "-otxt".to_string(),
        "-of".to_string(),
        output_base.to_string_lossy().to_string(),
    ];

    if options.language != "auto" {
        whisper_args.push("-l".to_string());
        whisper_args.push(options.language.clone());
    }

    if diarization_mode == "tdrz_2speaker" {
        whisper_args.push("-tdrz".to_string());
    }

    let whisper_output = run_whisper(&app, &whisper_args).await?;
    if !whisper_output.used_sidecar {
        warnings.push(
            "Using local whisper binary fallback in debug mode. Release builds use sidecar."
                .to_string(),
        );
    }

    if !whisper_output.success {
        return Err(format!(
            "Whisper failed: {}",
            String::from_utf8_lossy(&whisper_output.stderr)
        ));
    }

    emit_progress(&app, 85, "Reading transcript...");

    let transcript_raw = fs::read_to_string(&txt_temp_path).map_err(|e| {
        format!(
            "Whisper ran but transcript file could not be read ({}): {}",
            txt_temp_path.display(),
            e
        )
    })?;

    let (transcript, diarization_applied) = if diarization_mode == "tdrz_2speaker" {
        let (formatted, applied) = apply_tdrz_speaker_labels(&transcript_raw);
        if !applied {
            warnings.push(
                "2-speaker mode did not produce speaker boundaries. Output is unsegmented."
                    .to_string(),
            );
        }
        (formatted, applied)
    } else {
        (normalize_transcript(&transcript_raw), false)
    };

    if transcript.is_empty() {
        return Err("Whisper returned an empty transcript.".to_string());
    }

    let settings = load_settings(&app)?;
    let output_mode = validate_output_mode(&options.output_mode);
    let mut save_destination: Option<PathBuf> = None;

    if options.save_markdown {
        let now = now_local_or_utc();
        let date = format_date(now);
        let time_compact = format_time_compact(now);

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

    let duration_seconds = estimate_duration_seconds(&options.audio_data);
    let now = now_local_or_utc();
    let created_at = format_iso8601(now);
    let created_date = format_date(now);

    let frontmatter_client = sanitize_non_empty(options.client.clone())
        .or_else(|| sanitize_non_empty(settings.coachnotes_client.clone()));

    let markdown = build_markdown_transcript(
        &transcript,
        frontmatter_client.as_deref(),
        &options.model,
        &options.language,
        &diarization_mode,
        &created_at,
        &created_date,
        duration_seconds,
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

    let _ = fs::remove_file(&wav_path);
    let _ = fs::remove_file(&txt_temp_path);

    emit_progress(&app, 100, "Transcription complete!");

    Ok(TranscriptionResult {
        transcript,
        saved_path,
        format: "md".to_string(),
        diarization_applied,
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
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_setup_state,
            set_selected_model,
            set_transcript_directory,
            get_coachnotes_clients,
            set_coachnotes_settings,
            download_model,
            transcribe_recording,
            show_in_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
