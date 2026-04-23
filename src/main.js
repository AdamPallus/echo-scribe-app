const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const appWindow = window.__TAURI__.window?.getCurrentWindow?.() || null;

const modelSelect = document.getElementById('model-select');
const languageSelect = document.getElementById('language-select');
const diarizationModeSelect = document.getElementById('diarization-mode-select');
const saveMarkdownCheckbox = document.getElementById('save-markdown');
const saveRawAudioCheckbox = document.getElementById('save-raw-audio');
const setupDetails = document.getElementById('setup-details');
const captureOptionButtons = Array.from(document.querySelectorAll('[data-capture-option]'));

const setupPill = document.getElementById('setup-pill');
const setupMessage = document.getElementById('setup-message');
const sidecarStatus = document.getElementById('sidecar-status');
const modelStatusText = document.getElementById('model-status-text');
const downloadModelBtn = document.getElementById('download-model-btn');
const modelProgressWrap = document.getElementById('model-progress-wrap');
const modelProgressFill = document.getElementById('model-progress-fill');
const modelProgressText = document.getElementById('model-progress-text');
const transcriptDirInput = document.getElementById('transcript-dir');
const chooseDirBtn = document.getElementById('choose-dir-btn');

const coachnotesEnabledCheckbox = document.getElementById('coachnotes-enabled');
const coachnotesRootDirInput = document.getElementById('coachnotes-root-dir');
const chooseCoachnotesDirBtn = document.getElementById('choose-coachnotes-dir-btn');
const coachnotesClientSelect = document.getElementById('coachnotes-client-select');
const destinationPreview = document.getElementById('destination-preview');
const captureModeSelect = document.getElementById('capture-mode-select');
const captureModeHelp = document.getElementById('capture-mode-help');
const micMeterText = document.getElementById('mic-meter-text');
const micLevelFill = document.getElementById('mic-level-fill');
const systemMeterState = document.getElementById('system-meter-state');
const systemMeterText = document.getElementById('system-meter-text');
const overviewSource = document.getElementById('overview-source');
const overviewDuration = document.getElementById('overview-duration');
const overviewRecorded = document.getElementById('overview-recorded');
const overviewOutput = document.getElementById('overview-output');

const discardBtn = document.getElementById('discard-btn');
const discardBtnTitle = document.getElementById('discard-btn-title');
const startBtn = document.getElementById('start-btn');
const recordToggleTitle = document.getElementById('record-toggle-title');
const recordToggleIcon = document.getElementById('record-toggle-icon');
const transcribeBtn = document.getElementById('transcribe-btn');
const transcribeBtnTitle = document.getElementById('transcribe-btn-title');
const statusDot = document.querySelector('.status-dot');
const statusEl = document.getElementById('recording-status');
const timerEl = document.getElementById('recording-timer');
const progressSection = document.getElementById('progress-section');
const progressFill = document.getElementById('progress-fill');
const progressText = document.getElementById('progress-text');
const resultSection = document.getElementById('result-section');
const warningsList = document.getElementById('warnings-list');
const transcriptOutput = document.getElementById('transcript-output');
const openFileBtn = document.getElementById('open-file-btn');
const titlebar = document.getElementById('app-titlebar');

let setupState = null;
let modelDownloadInProgress = false;
const DIARIZATION_MODEL_ID = 'small.en-tdrz';

let captureStreams = [];
let audioContext = null;
let sourceNodes = [];
let processorNode = null;
let silentGain = null;
let recordingStartTime = null;
let timerInterval = null;
let isRecording = false;
let isStoppingRecording = false;
let isTranscribing = false;
let isSavingCoachnotesSettings = false;
let activeCaptureMode = 'microphone';
let systemCaptureActive = false;
let microphoneCaptureActive = false;
let audioChunks = [];
let totalSamples = 0;
let sampleRate = 44100;
let recordedCapture = null;
let savedTranscriptPath = null;
let savedAudioPaths = [];
let hasTranscriptionResult = false;
let recordingStartWallTimeMs = 0;
let lastRecordingAt = null;
const METER_FLOOR = 0;

function hasRecordedAudio() {
  return Boolean(
    recordedCapture
    && (
      recordedCapture.primaryWav
      || recordedCapture.microphoneWav
      || recordedCapture.systemWav
    )
  );
}

function currentPrimaryWav() {
  return recordedCapture?.primaryWav || null;
}

function setStatus(message, state = 'idle') {
  statusEl.textContent = message;
  statusEl.className = `status ${state}`;
  if (statusDot) {
    statusDot.className = `status-dot ${state}`;
  }
  refreshDashboardState();
}

function resetTimer() {
  timerEl.textContent = '00:00';
  updateOverview();
}

function startTimer() {
  recordingStartTime = Date.now();
  timerInterval = setInterval(() => {
    const elapsedMs = Date.now() - recordingStartTime;
    const totalSeconds = Math.floor(elapsedMs / 1000);
    const minutes = String(Math.floor(totalSeconds / 60)).padStart(2, '0');
    const seconds = String(totalSeconds % 60).padStart(2, '0');
    timerEl.textContent = `${minutes}:${seconds}`;
    updateOverview();
  }, 200);
}

function stopTimer() {
  if (timerInterval) {
    clearInterval(timerInterval);
    timerInterval = null;
  }
}

function formatBytes(bytes) {
  if (!bytes || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, index);
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function selectedModelEntry() {
  if (!setupState) return null;
  return setupState.models.find((entry) => entry.id === modelSelect.value) || null;
}

function selectedModelReady() {
  const entry = selectedModelEntry();
  return Boolean(entry && entry.downloaded);
}

function coachnotesEnabled() {
  return Boolean(coachnotesEnabledCheckbox.checked);
}

function getOutputMode() {
  return coachnotesEnabled() ? 'coachnotes' : 'standard';
}

function selectedCaptureMode() {
  return captureModeSelect.value || 'microphone';
}

function captureModeUsesMicrophone(mode) {
  return mode !== 'system';
}

function captureModeNeedsSystemAudio(mode) {
  return mode === 'system' || mode === 'both';
}

function describeCaptureMode(mode) {
  if (mode === 'system') return 'System Audio';
  if (mode === 'both') return 'System + Microphone';
  return 'Microphone';
}

function describeSpeakerMode(mode) {
  if (mode === 'source_aware_2speaker') return 'Source-aware';
  if (mode === 'tdrz_2speaker') return 'Whisper diarization';
  return 'Standard';
}

function basename(path) {
  if (!path) return '';
  const normalized = String(path).replace(/\\/g, '/');
  const parts = normalized.split('/');
  return parts[parts.length - 1] || normalized;
}

function formatRecordedAt(value) {
  if (!value) return 'Not recorded yet';

  try {
    return new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(value);
  } catch {
    return value.toLocaleString();
  }
}

function setMicLevel(level) {
  const percent = Math.max(METER_FLOOR, Math.min(1, level));
  micLevelFill.style.width = `${Math.round(percent * 100)}%`;
}

function resetMicMeter() {
  setMicLevel(0);
}

function syncCaptureOptionButtons() {
  const selected = selectedCaptureMode();
  for (const button of captureOptionButtons) {
    button.classList.toggle('is-selected', button.dataset.captureOption === selected);
  }
}

function measureAudioLevel(samples) {
  if (!samples || samples.length === 0) {
    return 0;
  }

  let squared = 0;
  let peak = 0;
  for (let i = 0; i < samples.length; i++) {
    const value = Math.abs(samples[i]);
    squared += value * value;
    if (value > peak) {
      peak = value;
    }
  }

  const rms = Math.sqrt(squared / samples.length);
  return Math.min(1, Math.max(rms * 4.8, peak * 0.72));
}

function currentOverviewOutput() {
  if (savedTranscriptPath) {
    return basename(savedTranscriptPath);
  }

  if (savedAudioPaths.length > 0) {
    return basename(savedAudioPaths[0]);
  }

  if (hasRecordedAudio()) {
    if (!saveMarkdownCheckbox.checked && !saveRawAudioCheckbox.checked) {
      return 'No file output enabled';
    }

    if (saveMarkdownCheckbox.checked && saveRawAudioCheckbox.checked) {
      return coachnotesEnabled() ? 'Ready for CoachNotes + WAV export' : 'Ready for transcript + WAV export';
    }

    if (saveMarkdownCheckbox.checked) {
      return coachnotesEnabled() ? 'Ready for CoachNotes export' : 'Ready for markdown export';
    }

    return 'Ready for WAV export';
  }

  return 'Nothing saved yet';
}

function updateCaptureIndicators() {
  const mode = selectedCaptureMode();
  const micEnabled = captureModeUsesMicrophone(mode);
  const systemEnabled = captureModeNeedsSystemAudio(mode);

  if (isRecording && microphoneCaptureActive) {
    micMeterText.textContent = 'Live';
  } else if (micEnabled) {
    micMeterText.textContent = 'Armed';
    resetMicMeter();
  } else {
    micMeterText.textContent = 'Off';
    resetMicMeter();
  }

  systemMeterState.className = 'system-pill';

  if (isRecording && systemCaptureActive) {
    systemMeterState.textContent = 'Live';
    systemMeterState.classList.add('is-live');
    systemMeterText.textContent =
      'Native system capture is recording in parallel with the current session.';
    return;
  }

  if (systemEnabled) {
    systemMeterState.textContent = 'Armed';
    systemMeterState.classList.add('is-armed');
    if (mode === 'both') {
      systemMeterText.textContent =
        'System audio is armed for source-aware capture when recording starts.';
    } else {
      systemMeterText.textContent =
        'System audio will be captured directly from macOS for this session.';
    }
    return;
  }

  systemMeterState.textContent = 'Off';
  systemMeterState.classList.add('is-off');
  systemMeterText.textContent =
    'System audio capture is not selected for this session.';
}

function updateOverview() {
  overviewSource.textContent = `${describeCaptureMode(selectedCaptureMode())} • ${describeSpeakerMode(currentSpeakerMode())}`;
  overviewDuration.textContent = timerEl.textContent;
  overviewRecorded.textContent = formatRecordedAt(lastRecordingAt);
  overviewOutput.textContent = currentOverviewOutput();
}

function refreshDashboardState() {
  syncCaptureOptionButtons();
  updateCaptureIndicators();
  updateOverview();
}

function updateCaptureModeHelp() {
  const mode = selectedCaptureMode();
  if (mode === 'system') {
    captureModeHelp.textContent =
      'Captures all system audio from this Mac (native ScreenCaptureKit).';
  } else if (mode === 'both') {
    captureModeHelp.textContent =
      'Captures system audio and microphone together (native system capture + mic mix).';
  } else {
    captureModeHelp.textContent = 'Records local microphone input from this Mac.';
  }

  refreshDashboardState();
}

async function applyModelSelection(modelId) {
  if (modelSelect.value === modelId) {
    return;
  }

  modelSelect.value = modelId;
  setupState = await invoke('set_selected_model', { model: modelId });
  renderSetupState();
}

function currentSpeakerMode() {
  return diarizationModeSelect.value || 'none';
}

async function ensureTwoSpeakerRequirements() {
  if (currentSpeakerMode() !== 'tdrz_2speaker') {
    return true;
  }

  try {
    let changed = false;

    if (languageSelect.value !== 'en') {
      languageSelect.value = 'en';
      changed = true;
    }

    if (modelSelect.value !== DIARIZATION_MODEL_ID) {
      await applyModelSelection(DIARIZATION_MODEL_ID);
      changed = true;
    }

    if (changed) {
      if (selectedModelReady()) {
        setStatus('2-speaker mode is active (small.en-tdrz + English).', 'idle');
      } else {
        setStatus('2-speaker mode selected. Download small.en-tdrz to continue.', 'warning');
      }
    }

    return true;
  } catch (error) {
    diarizationModeSelect.value = 'none';
    setStatus(`Could not enable 2-speaker mode: ${String(error)}`, 'error');
    return false;
  }
}

function getSelectedCoachnotesClient() {
  const value = String(coachnotesClientSelect.value || '').trim();
  return value.length > 0 ? value : null;
}

function updateDestinationPreview() {
  if (!saveMarkdownCheckbox.checked && !saveRawAudioCheckbox.checked) {
    destinationPreview.textContent = 'File output is disabled for this run.';
    refreshDashboardState();
    return;
  }

  const now = new Date();
  const date = now.toISOString().slice(0, 10);
  const hh = String(now.getHours()).padStart(2, '0');
  const mm = String(now.getMinutes()).padStart(2, '0');
  const ss = String(now.getSeconds()).padStart(2, '0');
  const transcriptDir = String(transcriptDirInput.value || '').trim();
  const rawAudioBase = `${date}-transcript-${hh}${mm}${ss}`;
  const lines = [];

  if (saveMarkdownCheckbox.checked && coachnotesEnabled()) {
    const root = String(coachnotesRootDirInput.value || '').trim();
    const client = getSelectedCoachnotesClient();
    if (!root || !client) {
      lines.push('CoachNotes mode: choose root folder and client to preview destination.');
    } else {
      lines.push(`Transcript: ${root}/${client}/${date}-transcript-${hh}${mm}${ss}.md`);
    }
  } else if (saveMarkdownCheckbox.checked) {
    if (!transcriptDir) {
      lines.push('Transcript: default transcript folder.');
    } else {
      lines.push(`Transcript: ${transcriptDir}/transcript-<timestamp>.md`);
    }
  }

  if (saveRawAudioCheckbox.checked) {
    if (!transcriptDir) {
      lines.push('Raw audio: default transcript folder.');
    } else if (selectedCaptureMode() === 'both') {
      lines.push(`Raw audio: ${transcriptDir}/${rawAudioBase}-recording.wav`);
      lines.push(`Also saves: ${rawAudioBase}-coach-mic.wav and ${rawAudioBase}-client-system.wav`);
    } else {
      lines.push(`Raw audio: ${transcriptDir}/${rawAudioBase}-recording.wav`);
    }
  }

  destinationPreview.textContent = lines.join('\n');
  refreshDashboardState();
}

function populateCoachnotesClients(clients, selectedClient) {
  coachnotesClientSelect.innerHTML = '';

  const placeholder = document.createElement('option');
  placeholder.value = '';
  placeholder.textContent = clients.length > 0 ? 'Select client folder' : 'No client folders found';
  coachnotesClientSelect.appendChild(placeholder);

  for (const client of clients) {
    const option = document.createElement('option');
    option.value = client;
    option.textContent = client;
    coachnotesClientSelect.appendChild(option);
  }

  if (selectedClient && clients.includes(selectedClient)) {
    coachnotesClientSelect.value = selectedClient;
  } else {
    coachnotesClientSelect.value = '';
  }
}

function renderWarnings(warnings) {
  const rows = Array.isArray(warnings) ? warnings.filter(Boolean) : [];
  warningsList.innerHTML = '';

  if (rows.length === 0) {
    warningsList.hidden = true;
    return;
  }

  for (const warning of rows) {
    const li = document.createElement('li');
    li.textContent = warning;
    warningsList.appendChild(li);
  }

  warningsList.hidden = false;
}

function syncActionButtons() {
  const modelReady = selectedModelReady();
  const setupReady = Boolean(setupState && setupState.ready);
  const canTranscribe = setupReady && modelReady;
  captureModeSelect.disabled =
    modelDownloadInProgress || isTranscribing || isRecording || isSavingCoachnotesSettings;
  for (const button of captureOptionButtons) {
    button.disabled =
      modelDownloadInProgress || isTranscribing || isRecording || isSavingCoachnotesSettings;
  }
  discardBtn.disabled = isRecording || isStoppingRecording || (!hasRecordedAudio() && !hasTranscriptionResult);

  if (isRecording) {
    startBtn.disabled = isStoppingRecording;
    startBtn.classList.add('is-recording');
    recordToggleTitle.textContent = 'Stop Recording';
    recordToggleIcon.setAttribute('href', '#icon-stop');
    transcribeBtn.disabled = true;
    transcribeBtnTitle.textContent = 'Transcribe';
    discardBtnTitle.textContent = 'Discard';
    refreshDashboardState();
    return;
  }

  startBtn.classList.remove('is-recording');
  recordToggleTitle.textContent = 'Start Recording';
  recordToggleIcon.setAttribute('href', '#icon-record');
  startBtn.disabled =
    modelDownloadInProgress || isTranscribing || isSavingCoachnotesSettings || !canTranscribe;
  transcribeBtn.disabled =
    modelDownloadInProgress ||
    isTranscribing ||
    isSavingCoachnotesSettings ||
    !hasRecordedAudio() ||
    !canTranscribe;

  transcribeBtnTitle.textContent = hasTranscriptionResult ? 'Transcribe Again' : 'Transcribe';
  discardBtnTitle.textContent = 'Discard';
  refreshDashboardState();
}

function renderSetupState() {
  if (!setupState) return;

  transcriptDirInput.value = setupState.transcript_dir;
  coachnotesEnabledCheckbox.checked = Boolean(setupState.coachnotes_enabled);
  coachnotesRootDirInput.value = setupState.coachnotes_root_dir || '';
  diarizationModeSelect.value = setupState.diarization_mode || 'none';

  populateCoachnotesClients(
    setupState.coachnotes_clients || [],
    setupState.coachnotes_client || ''
  );

  const entry = selectedModelEntry();
  if (!entry) {
    setupPill.textContent = 'Invalid model';
    setupPill.className = 'pill warning';
    setupMessage.textContent = 'Select a valid model to continue.';
    modelStatusText.textContent = '';
    downloadModelBtn.disabled = true;
    sidecarStatus.textContent = '';
    updateDestinationPreview();
    syncActionButtons();
    return;
  }

  modelStatusText.textContent = `${entry.label} | ~${entry.size_mb} MB`;

  if (setupState.sidecar_ready) {
    sidecarStatus.textContent = 'Bundled whisper sidecar detected.';
  } else {
    sidecarStatus.textContent =
      'Whisper sidecar not detected in this runtime. Debug fallback may use a local installation.';
  }

  if (entry.downloaded && setupState.ready) {
    setupPill.textContent = 'Ready';
    setupPill.className = 'pill ready';
    setupMessage.textContent = `Model '${entry.id}' is installed. You can record and transcribe locally.`;
    downloadModelBtn.textContent = 'Model Installed';
    downloadModelBtn.disabled = true;
  } else if (!entry.downloaded) {
    setupPill.textContent = 'Setup required';
    setupPill.className = 'pill warning';
    setupMessage.textContent = `Model '${entry.id}' is not downloaded yet.`;
    downloadModelBtn.textContent = 'Download Selected Model';
    downloadModelBtn.disabled = modelDownloadInProgress;
  } else {
    setupPill.textContent = 'Runtime issue';
    setupPill.className = 'pill warning';
    setupMessage.textContent =
      'Model is available but transcription runtime is not ready. Check sidecar setup.';
    downloadModelBtn.textContent = 'Model Installed';
    downloadModelBtn.disabled = true;
  }

  const coachEnabled = coachnotesEnabled();
  chooseDirBtn.disabled = modelDownloadInProgress || isTranscribing;
  chooseCoachnotesDirBtn.disabled = modelDownloadInProgress || isTranscribing || !coachEnabled;
  coachnotesClientSelect.disabled = modelDownloadInProgress || isTranscribing || !coachEnabled;

  if (setupDetails && (modelDownloadInProgress || !setupState.ready)) {
    setupDetails.open = true;
  }

  updateDestinationPreview();
  syncActionButtons();
}

async function refreshSetupState() {
  setupState = await invoke('get_setup_state');
  modelSelect.value = setupState.selected_model;
  renderSetupState();
}

async function saveDiarizationMode(mode) {
  setupState = await invoke('set_diarization_mode', { mode });
  renderSetupState();
}

async function saveCoachnotesSettings() {
  const input = {
    enabled: coachnotesEnabled(),
    root_dir: coachnotesRootDirInput.value || null,
    client: getSelectedCoachnotesClient(),
  };

  isSavingCoachnotesSettings = true;
  syncActionButtons();

  try {
    setupState = await invoke('set_coachnotes_settings', { input });
    renderSetupState();
  } finally {
    isSavingCoachnotesSettings = false;
    syncActionButtons();
  }
}

function mergeChunks(chunks, length) {
  const merged = new Float32Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return merged;
}

function downsampleBuffer(buffer, inputRate, outputRate) {
  if (outputRate >= inputRate) {
    return buffer;
  }

  const ratio = inputRate / outputRate;
  const outputLength = Math.round(buffer.length / ratio);
  const output = new Float32Array(outputLength);

  let offsetResult = 0;
  let offsetBuffer = 0;

  while (offsetResult < output.length) {
    const nextOffsetBuffer = Math.round((offsetResult + 1) * ratio);
    let accum = 0;
    let count = 0;

    for (let i = offsetBuffer; i < nextOffsetBuffer && i < buffer.length; i++) {
      accum += buffer[i];
      count += 1;
    }

    output[offsetResult] = count > 0 ? accum / count : 0;
    offsetResult += 1;
    offsetBuffer = nextOffsetBuffer;
  }

  return output;
}

function encodeWav(samples, rate) {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);

  function writeString(offset, string) {
    for (let i = 0; i < string.length; i++) {
      view.setUint8(offset + i, string.charCodeAt(i));
    }
  }

  writeString(0, 'RIFF');
  view.setUint32(4, 36 + samples.length * 2, true);
  writeString(8, 'WAVE');
  writeString(12, 'fmt ');
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeString(36, 'data');
  view.setUint32(40, samples.length * 2, true);

  let offset = 44;
  for (let i = 0; i < samples.length; i++) {
    const sample = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(offset, sample < 0 ? sample * 0x8000 : sample * 0x7fff, true);
    offset += 2;
  }

  return new Uint8Array(buffer);
}

async function cleanupRecordingGraph() {
  for (const node of sourceNodes) {
    node.disconnect();
  }
  sourceNodes = [];

  if (processorNode) {
    processorNode.disconnect();
    processorNode.onaudioprocess = null;
    processorNode = null;
  }
  if (silentGain) {
    silentGain.disconnect();
    silentGain = null;
  }
  for (const stream of captureStreams) {
    for (const track of stream.getTracks()) {
      track.stop();
    }
  }
  captureStreams = [];

  if (audioContext) {
    await audioContext.close();
    audioContext = null;
  }
}

function extractMonoChannel(inputBuffer) {
  const channels = inputBuffer.numberOfChannels;
  const length = inputBuffer.length;

  if (channels === 0) {
    return new Float32Array(length);
  }

  if (channels <= 1) {
    return inputBuffer.getChannelData(0);
  }

  const mixed = new Float32Array(length);
  for (let channelIndex = 0; channelIndex < channels; channelIndex++) {
    const channel = inputBuffer.getChannelData(channelIndex);
    for (let i = 0; i < length; i++) {
      mixed[i] += channel[i];
    }
  }

  for (let i = 0; i < length; i++) {
    mixed[i] /= channels;
  }

  return mixed;
}

function registerTrackEndHandlers(stream, label) {
  for (const track of stream.getTracks()) {
    track.addEventListener('ended', () => {
      if (isRecording && !isStoppingRecording) {
        setStatus(`${label} capture ended. Recording stopped.`, 'warning');
        void stopRecording();
      }
    });
  }
}

function decodePcm16Wav(bytes) {
  const data = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);

  const readStr = (offset, len) => {
    let out = '';
    for (let i = 0; i < len; i++) out += String.fromCharCode(view.getUint8(offset + i));
    return out;
  };

  if (data.byteLength < 44 || readStr(0, 4) !== 'RIFF' || readStr(8, 4) !== 'WAVE') {
    throw new Error('Invalid WAV data.');
  }

  let offset = 12;
  let format = null;
  let resolvedFormat = null;
  let channels = 1;
  let sampleRateHz = 16000;
  let bitsPerSample = 16;
  let dataOffset = -1;
  let dataSize = 0;

  while (offset + 8 <= data.byteLength) {
    const chunkId = readStr(offset, 4);
    const chunkSize = view.getUint32(offset + 4, true);
    const chunkStart = offset + 8;

    if (chunkId === 'fmt ' && chunkSize >= 16 && chunkStart + chunkSize <= data.byteLength) {
      format = view.getUint16(chunkStart + 0, true);
      channels = view.getUint16(chunkStart + 2, true);
      sampleRateHz = view.getUint32(chunkStart + 4, true);
      bitsPerSample = view.getUint16(chunkStart + 14, true);

      if (format === 0xfffe && chunkSize >= 40) {
        const subFormat = view.getUint32(chunkStart + 24, true);
        if (subFormat === 1 || subFormat === 3) {
          resolvedFormat = subFormat;
        }
      } else {
        resolvedFormat = format;
      }
    } else if (chunkId === 'data' && chunkStart + chunkSize <= data.byteLength) {
      dataOffset = chunkStart;
      dataSize = chunkSize;
      break;
    }

    offset = chunkStart + chunkSize + (chunkSize % 2);
  }

  const effectiveFormat = resolvedFormat ?? format;
  const isPcm16 = effectiveFormat === 1 && bitsPerSample === 16;
  const isFloat32 = effectiveFormat === 3 && bitsPerSample === 32;

  if ((!isPcm16 && !isFloat32) || dataOffset < 0 || channels <= 0) {
    throw new Error(
      `Unsupported WAV format. Expected PCM16/Float32, got format=${effectiveFormat ?? 'unknown'} bits=${bitsPerSample}.`
    );
  }

  const bytesPerSample = bitsPerSample / 8;
  const frameCount = Math.floor(dataSize / (bytesPerSample * channels));
  const samples = new Float32Array(frameCount);

  let cursor = dataOffset;
  for (let frame = 0; frame < frameCount; frame++) {
    let mixed = 0;
    for (let channel = 0; channel < channels; channel++) {
      let sample = 0;
      if (isPcm16) {
        sample = view.getInt16(cursor, true) / 32768;
      } else if (isFloat32) {
        sample = view.getFloat32(cursor, true);
      }
      cursor += bytesPerSample;
      mixed += Number.isFinite(sample) ? sample : 0;
    }
    samples[frame] = Math.max(-1, Math.min(1, mixed / channels));
  }

  return {
    samples,
    sampleRate: sampleRateHz,
  };
}

function resampleBuffer(buffer, inputRate, outputRate) {
  if (inputRate === outputRate || buffer.length === 0) {
    return buffer;
  }

  if (inputRate > outputRate) {
    return downsampleBuffer(buffer, inputRate, outputRate);
  }

  const ratio = outputRate / inputRate;
  const outputLength = Math.max(1, Math.round(buffer.length * ratio));
  const output = new Float32Array(outputLength);

  for (let i = 0; i < outputLength; i++) {
    const sourceIndex = i / ratio;
    const lower = Math.floor(sourceIndex);
    const upper = Math.min(lower + 1, buffer.length - 1);
    const frac = sourceIndex - lower;
    output[i] = buffer[lower] * (1 - frac) + buffer[upper] * frac;
  }

  return output;
}

function normalizeWavTo16k(wavBytes) {
  const decoded = decodePcm16Wav(wavBytes);
  const normalized = resampleBuffer(decoded.samples, decoded.sampleRate, 16000);
  return encodeWav(normalized, 16000);
}

function mergeSystemAndMic(systemWav, micWav) {
  const systemDecoded = decodePcm16Wav(systemWav);
  const micDecoded = decodePcm16Wav(micWav);
  const targetRate = 16000;

  const systemSamples = resampleBuffer(systemDecoded.samples, systemDecoded.sampleRate, targetRate);
  const micSamples = resampleBuffer(micDecoded.samples, micDecoded.sampleRate, targetRate);

  const mixedLength = Math.max(systemSamples.length, micSamples.length);
  const mixed = new Float32Array(mixedLength);

  for (let i = 0; i < mixedLength; i++) {
    const a = i < systemSamples.length ? systemSamples[i] : 0;
    const b = i < micSamples.length ? micSamples[i] : 0;
    mixed[i] = Math.max(-1, Math.min(1, (a + b) * 0.5));
  }

  return encodeWav(mixed, targetRate);
}

function buildAlignmentSignal(samples, inputRate) {
  const targetRate = 50;
  const absolute = new Float32Array(samples.length);
  for (let i = 0; i < samples.length; i++) {
    absolute[i] = Math.abs(samples[i]);
  }

  const resampled = resampleBuffer(absolute, inputRate, targetRate);
  const smoothed = new Float32Array(resampled.length);
  const windowRadius = 2;
  for (let i = 0; i < resampled.length; i++) {
    let total = 0;
    let count = 0;
    const start = Math.max(0, i - windowRadius);
    const end = Math.min(resampled.length - 1, i + windowRadius);
    for (let j = start; j <= end; j++) {
      total += resampled[j];
      count += 1;
    }
    smoothed[i] = count > 0 ? total / count : 0;
  }

  return { signal: smoothed, rate: targetRate };
}

function estimateSystemAlignmentOffsetMs(systemWav, micWav) {
  if (!systemWav || !micWav) {
    return null;
  }

  const systemDecoded = decodePcm16Wav(systemWav);
  const micDecoded = decodePcm16Wav(micWav);
  const { signal: rawSystemSignal, rate } = buildAlignmentSignal(systemDecoded.samples, systemDecoded.sampleRate);
  const { signal: rawMicSignal } = buildAlignmentSignal(micDecoded.samples, micDecoded.sampleRate);

  const maxSystemSamples = Math.min(rawSystemSignal.length, rate * 30);
  const maxMicSamples = Math.min(rawMicSignal.length, rate * 120);
  const systemSignal = rawSystemSignal.subarray(0, maxSystemSamples);
  const micSignal = rawMicSignal.subarray(0, maxMicSamples);

  if (systemSignal.length < rate * 2 || micSignal.length <= systemSignal.length) {
    return null;
  }

  const micSquares = new Float64Array(micSignal.length + 1);
  for (let i = 0; i < micSignal.length; i++) {
    micSquares[i + 1] = micSquares[i] + (micSignal[i] * micSignal[i]);
  }

  let systemEnergy = 0;
  for (let i = 0; i < systemSignal.length; i++) {
    systemEnergy += systemSignal[i] * systemSignal[i];
  }
  if (systemEnergy <= 1e-9) {
    return null;
  }

  let bestLagSamples = 0;
  let bestScore = -Infinity;
  const maxLag = micSignal.length - systemSignal.length;
  for (let lag = 0; lag <= maxLag; lag++) {
    let dot = 0;
    for (let i = 0; i < systemSignal.length; i++) {
      dot += micSignal[lag + i] * systemSignal[i];
    }

    const micEnergy = micSquares[lag + systemSignal.length] - micSquares[lag];
    if (micEnergy <= 1e-9) {
      continue;
    }

    const score = dot / Math.sqrt(systemEnergy * micEnergy);
    if (score > bestScore) {
      bestScore = score;
      bestLagSamples = lag;
    }
  }

  if (!Number.isFinite(bestScore) || bestScore < 0.2) {
    return null;
  }

  return Math.round((bestLagSamples * 1000) / rate);
}

function shouldUseSourceAwareMicProcessing() {
  return selectedCaptureMode() === 'both' && currentSpeakerMode() === 'source_aware_2speaker';
}

async function requestMicrophoneStream() {
  const sourceAwareProcessing = shouldUseSourceAwareMicProcessing();
  return navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: sourceAwareProcessing,
      noiseSuppression: sourceAwareProcessing,
      autoGainControl: false,
      channelCount: 1,
    },
  });
}

async function startRecording() {
  if (!selectedModelReady()) {
    setStatus('Download the selected model first.', 'error');
    return;
  }

  const mode = selectedCaptureMode();
  const captureSystemAudio = captureModeNeedsSystemAudio(mode);
  const captureMic = mode !== 'system';

  try {
    if (captureSystemAudio) {
      await invoke('start_system_audio_recording');
      systemCaptureActive = true;
    } else {
      systemCaptureActive = false;
    }

    const streams = [];
    if (captureMic) {
      const micStream = await requestMicrophoneStream();
      registerTrackEndHandlers(micStream, 'Microphone');
      streams.push(micStream);
    }
    microphoneCaptureActive = captureMic;
    captureStreams = streams;
    activeCaptureMode = mode;

    if (captureMic) {
      audioContext = new AudioContext();
      sampleRate = audioContext.sampleRate;
      processorNode = audioContext.createScriptProcessor(4096, 1, 1);
      silentGain = audioContext.createGain();
      silentGain.gain.value = 0;

      sourceNodes = captureStreams.map((stream) => {
        const node = audioContext.createMediaStreamSource(stream);
        node.connect(processorNode);
        return node;
      });

      processorNode.onaudioprocess = (event) => {
        if (!isRecording) return;
        const input = extractMonoChannel(event.inputBuffer);
        const copy = new Float32Array(input.length);
        copy.set(input);
        audioChunks.push(copy);
        totalSamples += copy.length;
        setMicLevel(measureAudioLevel(input));
      };

      processorNode.connect(silentGain);
      silentGain.connect(audioContext.destination);
    }

    audioChunks = [];
    totalSamples = 0;
    recordedCapture = null;
    hasTranscriptionResult = false;
    savedTranscriptPath = null;
    savedAudioPaths = [];
    resetMicMeter();

    recordingStartWallTimeMs = Date.now();
    lastRecordingAt = new Date(recordingStartWallTimeMs);
    isRecording = true;
    isStoppingRecording = false;
    openFileBtn.hidden = true;
    resultSection.hidden = true;
    progressSection.hidden = true;

    if (mode === 'system') {
      setStatus('Recording system audio...', 'recording');
    } else if (mode === 'both') {
      setStatus('Recording system + microphone...', 'recording');
    } else {
      setStatus('Recording microphone...', 'recording');
    }

    startTimer();
    syncActionButtons();
  } catch (error) {
    console.error(error);
    if (systemCaptureActive) {
      try {
        await invoke('stop_system_audio_recording');
      } catch {
        // ignore cleanup failure
      }
      systemCaptureActive = false;
    }
    microphoneCaptureActive = false;
    await cleanupRecordingGraph();
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`Capture error: ${message}`, 'error');
    syncActionButtons();
  }
}

async function stopRecording() {
  if (!isRecording || isStoppingRecording) return;

  isStoppingRecording = true;
  isRecording = false;
  stopTimer();

  try {
    let micWav = null;
    let systemWav = null;
    let primaryWav = null;
    let stopWarnings = [];
    let systemAudioOffsetMs = 0;

    if (microphoneCaptureActive) {
      await cleanupRecordingGraph();
      if (totalSamples > 0) {
        const merged = mergeChunks(audioChunks, totalSamples);
        const downsampled = downsampleBuffer(merged, sampleRate, 16000);
        micWav = encodeWav(downsampled, 16000);
      }
    } else {
      await cleanupRecordingGraph();
    }

    audioChunks = [];
    totalSamples = 0;

    if (systemCaptureActive) {
      try {
        const rawBytes = await invoke('stop_system_audio_recording');
        const audioData = Array.isArray(rawBytes?.audio_data)
          ? rawBytes.audio_data
          : rawBytes;
        systemWav = normalizeWavTo16k(Uint8Array.from(audioData || []));
        const firstAudioWallTimeMs = Number(rawBytes?.first_audio_wall_time_ms || 0);
        if (firstAudioWallTimeMs > 0 && recordingStartWallTimeMs > 0) {
          systemAudioOffsetMs = Math.max(0, firstAudioWallTimeMs - recordingStartWallTimeMs);
        }
      } catch (error) {
        if (activeCaptureMode === 'system') {
          throw error;
        }
        const message = error instanceof Error ? error.message : String(error);
        stopWarnings.push(`System audio capture failed: ${message}. Using microphone capture only.`);
      }
    }
    systemCaptureActive = false;
    microphoneCaptureActive = false;

    if (activeCaptureMode === 'system') {
      primaryWav = systemWav;
    } else if (activeCaptureMode === 'both') {
      if (systemWav && micWav) {
        primaryWav = mergeSystemAndMic(systemWav, micWav);
      } else {
        primaryWav = systemWav || micWav;
      }
    } else {
      primaryWav = micWav;
    }

    if (!primaryWav || primaryWav.length <= 44) {
      recordedCapture = null;
      setStatus('No audio captured. Try again.', 'error');
      syncActionButtons();
      return;
    }

    if (systemWav && micWav) {
      const estimatedAlignmentOffsetMs = estimateSystemAlignmentOffsetMs(systemWav, micWav);
      if (Number.isFinite(estimatedAlignmentOffsetMs)) {
        systemAudioOffsetMs = estimatedAlignmentOffsetMs;
      }
    }

    recordedCapture = {
      captureMode: activeCaptureMode,
      primaryWav,
      microphoneWav: micWav,
      systemWav,
      systemAudioOffsetMs: Number.isFinite(systemAudioOffsetMs) ? systemAudioOffsetMs : 0,
    };
    hasTranscriptionResult = false;
    if (stopWarnings.length > 0) {
      setStatus(`Recording ready to transcribe. ${stopWarnings.join(' ')}`, 'warning');
    } else {
      setStatus('Recording ready to transcribe', 'ready');
    }
    syncActionButtons();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    setStatus(`Stop error: ${message}`, 'error');
    syncActionButtons();
  } finally {
    isStoppingRecording = false;
  }
}

async function transcribeRecording() {
  const primaryWav = currentPrimaryWav();
  if (!primaryWav) return;
  if (!selectedModelReady()) {
    setStatus('Selected model is not downloaded.', 'error');
    return;
  }
  if (!(await ensureTwoSpeakerRequirements())) {
    return;
  }

  progressSection.hidden = false;
  resultSection.hidden = true;
  progressFill.style.width = '0%';
  progressText.textContent = 'Starting transcription...';
  setStatus('Transcribing locally...', 'working');
  isTranscribing = true;
  syncActionButtons();

  const options = {
    audio_data: Array.from(primaryWav),
    microphone_audio_data: recordedCapture?.microphoneWav ? Array.from(recordedCapture.microphoneWav) : [],
    system_audio_data: recordedCapture?.systemWav ? Array.from(recordedCapture.systemWav) : [],
    system_audio_offset_ms: recordedCapture?.systemAudioOffsetMs || 0,
    model: modelSelect.value,
    language: languageSelect.value,
    save_markdown: saveMarkdownCheckbox.checked,
    save_raw_audio: saveRawAudioCheckbox.checked,
    output_mode: getOutputMode(),
    client: getSelectedCoachnotesClient(),
    diarization_mode: currentSpeakerMode(),
  };

  try {
    const result = await invoke('transcribe_recording', { options });
    transcriptOutput.textContent = result.transcript || '';
    renderWarnings(result.warnings || []);
    resultSection.hidden = false;
    savedTranscriptPath = result.saved_path || null;
    savedAudioPaths = Array.isArray(result.saved_audio_paths)
      ? result.saved_audio_paths.filter(Boolean)
      : [];

    if (savedTranscriptPath || savedAudioPaths.length > 0) {
      openFileBtn.hidden = false;
    } else {
      openFileBtn.hidden = true;
    }

    if (result.speaker_mode_used === 'source_aware_2speaker') {
      setStatus(
        savedAudioPaths.length > 0
          ? 'Transcription complete (source-aware 2-speaker, raw audio saved)'
          : 'Transcription complete (source-aware 2-speaker)',
        'ready'
      );
    } else if (result.diarization_applied) {
      setStatus(
        savedAudioPaths.length > 0
          ? 'Transcription complete (2-speaker mode, raw audio saved)'
          : 'Transcription complete (2-speaker mode)',
        'ready'
      );
    } else {
      setStatus(
        savedAudioPaths.length > 0
          ? 'Transcription complete (raw audio saved)'
          : 'Transcription complete',
        'ready'
      );
    }
    hasTranscriptionResult = true;
  } catch (error) {
    renderWarnings([]);
    savedAudioPaths = [];
    setStatus(`Transcription failed: ${String(error)}`, 'error');
  } finally {
    isTranscribing = false;
    syncActionButtons();
  }
}

function discardCurrentSession() {
  if (isRecording || isStoppingRecording) {
    return;
  }

  recordedCapture = null;
  savedTranscriptPath = null;
  savedAudioPaths = [];
  hasTranscriptionResult = false;
  lastRecordingAt = null;
  transcriptOutput.textContent = '';
  renderWarnings([]);
  progressSection.hidden = true;
  resultSection.hidden = true;
  openFileBtn.hidden = true;
  resetTimer();
  resetMicMeter();
  setStatus('Ready to record', 'idle');
  syncActionButtons();
}

modelSelect.addEventListener('change', async () => {
  try {
    setupState = await invoke('set_selected_model', { model: modelSelect.value });
    renderSetupState();

    if (diarizationModeSelect.value === 'tdrz_2speaker' && modelSelect.value !== DIARIZATION_MODEL_ID) {
      await saveDiarizationMode('none');
      setStatus('2-speaker mode disabled. It requires the small.en-tdrz model.', 'warning');
    }
  } catch (error) {
    setStatus(`Failed to update model: ${String(error)}`, 'error');
  }
});

chooseDirBtn.addEventListener('click', async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: transcriptDirInput.value || undefined,
    });

    if (typeof selected !== 'string' || selected.length === 0) {
      return;
    }

    setupState = await invoke('set_transcript_directory', { directory: selected });
    renderSetupState();
  } catch (error) {
    setStatus(`Failed to set transcript folder: ${String(error)}`, 'error');
  }
});

coachnotesEnabledCheckbox.addEventListener('change', async () => {
  try {
    await saveCoachnotesSettings();
  } catch (error) {
    setStatus(`Failed to update CoachNotes mode: ${String(error)}`, 'error');
  }
});

chooseCoachnotesDirBtn.addEventListener('click', async () => {
  try {
    const selected = await open({
      directory: true,
      multiple: false,
      defaultPath: coachnotesRootDirInput.value || undefined,
    });

    if (typeof selected !== 'string' || selected.length === 0) {
      return;
    }

    coachnotesRootDirInput.value = selected;

    try {
      const clients = await invoke('get_coachnotes_clients', { rootDir: selected });
      populateCoachnotesClients(clients || [], coachnotesClientSelect.value || '');
    } catch {
      // keep going - set_coachnotes_settings will validate and return canonical state
    }

    await saveCoachnotesSettings();
  } catch (error) {
    setStatus(`Failed to set CoachNotes folder: ${String(error)}`, 'error');
  }
});

coachnotesClientSelect.addEventListener('change', async () => {
  try {
    await saveCoachnotesSettings();
  } catch (error) {
    setStatus(`Failed to set CoachNotes client: ${String(error)}`, 'error');
  }
});

downloadModelBtn.addEventListener('click', async () => {
  const entry = selectedModelEntry();
  if (!entry || entry.downloaded || modelDownloadInProgress) {
    return;
  }

  modelDownloadInProgress = true;
  modelProgressWrap.hidden = false;
  modelProgressFill.style.width = '0%';
  modelProgressText.textContent = `Preparing ${entry.id} model download...`;
  renderSetupState();

  try {
    await invoke('download_model', { options: { model: entry.id } });
    setupState = await invoke('get_setup_state');
    setStatus('Model downloaded. You can start recording.', 'ready');
  } catch (error) {
    setStatus(`Model download failed: ${String(error)}`, 'error');
  } finally {
    modelDownloadInProgress = false;
    renderSetupState();
  }
});

saveMarkdownCheckbox.addEventListener('change', () => {
  updateDestinationPreview();
  syncActionButtons();
});

saveRawAudioCheckbox.addEventListener('change', () => {
  updateDestinationPreview();
  syncActionButtons();
});

languageSelect.addEventListener('change', async () => {
  if (diarizationModeSelect.value === 'tdrz_2speaker') {
    await ensureTwoSpeakerRequirements();
  }
});

captureModeSelect.addEventListener('change', () => {
  updateCaptureModeHelp();
  updateDestinationPreview();
});

for (const button of captureOptionButtons) {
  button.addEventListener('click', () => {
    const nextMode = button.dataset.captureOption;
    if (!nextMode || nextMode === captureModeSelect.value) {
      return;
    }

    captureModeSelect.value = nextMode;
    captureModeSelect.dispatchEvent(new Event('change'));
  });
}

diarizationModeSelect.addEventListener('change', async () => {
  const nextMode = diarizationModeSelect.value;

  if (nextMode === 'tdrz_2speaker') {
    const enabled = await ensureTwoSpeakerRequirements();
    if (!enabled) {
      await saveDiarizationMode('none');
      return;
    }
  }

  try {
    await saveDiarizationMode(nextMode);
  } catch (error) {
    setStatus(`Failed to update speaker mode: ${String(error)}`, 'error');
    return;
  }

  if (nextMode === 'source_aware_2speaker') {
    setStatus('Source-aware 2-speaker mode selected. Use System audio + microphone.', 'idle');
  } else if (nextMode === 'tdrz_2speaker') {
    if (selectedModelReady()) {
      setStatus('2-speaker mode is active (small.en-tdrz + English).', 'idle');
    } else {
      setStatus('2-speaker mode selected. Download small.en-tdrz to continue.', 'warning');
    }
  } else {
    setStatus('Standard transcription mode selected.', 'idle');
  }
});

startBtn.addEventListener('click', () => {
  if (isRecording) {
    void stopRecording();
    return;
  }

  void startRecording();
});
transcribeBtn.addEventListener('click', transcribeRecording);
discardBtn.addEventListener('click', discardCurrentSession);

openFileBtn.addEventListener('click', async () => {
  const pathToShow = savedTranscriptPath || savedAudioPaths[0];
  if (!pathToShow) return;
  try {
    await invoke('show_in_folder', { path: pathToShow });
  } catch (error) {
    setStatus(`Could not open saved file: ${String(error)}`, 'error');
  }
});

if (titlebar && appWindow && typeof appWindow.startDragging === 'function') {
  titlebar.addEventListener('mousedown', (event) => {
    if (event.button !== 0) {
      return;
    }

    if (event.target.closest('button, input, select, textarea, a, [data-no-drag]')) {
      return;
    }

    void appWindow.startDragging().catch(() => {});
  });
}

listen('progress', (event) => {
  const { percent, message } = event.payload;
  progressFill.style.width = `${percent}%`;
  progressText.textContent = message;
});

listen('model-download-progress', (event) => {
  const payload = event.payload;
  const percent = Math.max(0, Math.min(100, payload.percent || 0));
  const total = payload.total_bytes ? formatBytes(payload.total_bytes) : 'unknown size';
  const downloaded = formatBytes(payload.downloaded_bytes || 0);

  modelProgressWrap.hidden = false;
  modelProgressFill.style.width = `${percent}%`;
  modelProgressText.textContent = `${payload.message} (${downloaded} / ${total})`;
});

async function boot() {
  resetTimer();
  resetMicMeter();
  updateCaptureModeHelp();
  savedAudioPaths = [];
  hasTranscriptionResult = false;
  setStatus('Ready to record', 'idle');

  try {
    await refreshSetupState();
    if (!selectedModelReady()) {
      setStatus('Download a model to begin.', 'idle');
    }
  } catch (error) {
    setStatus(`Setup load failed: ${String(error)}`, 'error');
  }
}

boot();
