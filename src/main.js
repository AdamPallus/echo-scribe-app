const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;

const modelSelect = document.getElementById('model-select');
const languageSelect = document.getElementById('language-select');
const diarizationModeSelect = document.getElementById('diarization-mode-select');
const saveMarkdownCheckbox = document.getElementById('save-markdown');

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

const startBtn = document.getElementById('start-btn');
const stopBtn = document.getElementById('stop-btn');
const transcribeBtn = document.getElementById('transcribe-btn');
const statusEl = document.getElementById('recording-status');
const timerEl = document.getElementById('recording-timer');
const progressSection = document.getElementById('progress-section');
const progressFill = document.getElementById('progress-fill');
const progressText = document.getElementById('progress-text');
const resultSection = document.getElementById('result-section');
const warningsList = document.getElementById('warnings-list');
const transcriptOutput = document.getElementById('transcript-output');
const openFileBtn = document.getElementById('open-file-btn');

let setupState = null;
let modelDownloadInProgress = false;

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
let recordedWav = null;
let savedTranscriptPath = null;

function setStatus(message, state = 'idle') {
  statusEl.textContent = message;
  statusEl.className = `status ${state}`;
}

function resetTimer() {
  timerEl.textContent = '00:00';
}

function startTimer() {
  recordingStartTime = Date.now();
  timerInterval = setInterval(() => {
    const elapsedMs = Date.now() - recordingStartTime;
    const totalSeconds = Math.floor(elapsedMs / 1000);
    const minutes = String(Math.floor(totalSeconds / 60)).padStart(2, '0');
    const seconds = String(totalSeconds % 60).padStart(2, '0');
    timerEl.textContent = `${minutes}:${seconds}`;
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

function captureModeNeedsSystemAudio(mode) {
  return mode === 'system' || mode === 'both';
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
}

function getSelectedCoachnotesClient() {
  const value = String(coachnotesClientSelect.value || '').trim();
  return value.length > 0 ? value : null;
}

function updateDestinationPreview() {
  if (!saveMarkdownCheckbox.checked) {
    destinationPreview.textContent = 'Markdown file output is disabled for this run.';
    return;
  }

  const now = new Date();
  const date = now.toISOString().slice(0, 10);
  const hh = String(now.getHours()).padStart(2, '0');
  const mm = String(now.getMinutes()).padStart(2, '0');
  const ss = String(now.getSeconds()).padStart(2, '0');

  if (coachnotesEnabled()) {
    const root = String(coachnotesRootDirInput.value || '').trim();
    const client = getSelectedCoachnotesClient();
    if (!root || !client) {
      destinationPreview.textContent =
        'CoachNotes mode: choose root folder and client to preview destination.';
      return;
    }

    destinationPreview.textContent = `Destination: ${root}/${client}/${date}-transcript-${hh}${mm}${ss}.md`;
    return;
  }

  const transcriptDir = String(transcriptDirInput.value || '').trim();
  if (!transcriptDir) {
    destinationPreview.textContent = 'Destination: default transcript folder.';
    return;
  }

  destinationPreview.textContent = `Destination: ${transcriptDir}/transcript-<timestamp>.md`;
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

  if (isRecording) {
    startBtn.disabled = true;
    stopBtn.disabled = false;
    transcribeBtn.disabled = true;
    return;
  }

  stopBtn.disabled = true;
  startBtn.disabled =
    modelDownloadInProgress || isTranscribing || isSavingCoachnotesSettings || !canTranscribe;
  transcribeBtn.disabled =
    modelDownloadInProgress ||
    isTranscribing ||
    isSavingCoachnotesSettings ||
    !recordedWav ||
    !canTranscribe;
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

  updateDestinationPreview();
  syncActionButtons();
}

async function refreshSetupState() {
  setupState = await invoke('get_setup_state');
  modelSelect.value = setupState.selected_model;
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
    } else if (chunkId === 'data' && chunkStart + chunkSize <= data.byteLength) {
      dataOffset = chunkStart;
      dataSize = chunkSize;
      break;
    }

    offset = chunkStart + chunkSize + (chunkSize % 2);
  }

  if (format !== 1 || bitsPerSample !== 16 || dataOffset < 0 || channels <= 0) {
    throw new Error('Unsupported WAV format. Expected PCM16.');
  }

  const frameCount = Math.floor(dataSize / (2 * channels));
  const samples = new Float32Array(frameCount);

  let cursor = dataOffset;
  for (let frame = 0; frame < frameCount; frame++) {
    let mixed = 0;
    for (let channel = 0; channel < channels; channel++) {
      const sample = view.getInt16(cursor, true);
      cursor += 2;
      mixed += sample / 32768;
    }
    samples[frame] = mixed / channels;
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

async function requestMicrophoneStream() {
  return navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: false,
      noiseSuppression: false,
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
      };

      processorNode.connect(silentGain);
      silentGain.connect(audioContext.destination);
    }

    audioChunks = [];
    totalSamples = 0;
    recordedWav = null;
    savedTranscriptPath = null;

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
        systemWav = normalizeWavTo16k(Uint8Array.from(rawBytes || []));
      } catch (error) {
        if (activeCaptureMode === 'system') {
          throw error;
        }
        const message = error instanceof Error ? error.message : String(error);
        setStatus(`System audio warning: ${message}. Using microphone capture only.`, 'warning');
      }
    }
    systemCaptureActive = false;
    microphoneCaptureActive = false;

    if (activeCaptureMode === 'system') {
      recordedWav = systemWav;
    } else if (activeCaptureMode === 'both') {
      if (systemWav && micWav) {
        recordedWav = mergeSystemAndMic(systemWav, micWav);
      } else {
        recordedWav = systemWav || micWav;
      }
    } else {
      recordedWav = micWav;
    }

    if (!recordedWav || recordedWav.length <= 44) {
      setStatus('No audio captured. Try again.', 'error');
      syncActionButtons();
      return;
    }

    setStatus('Recording ready to transcribe', 'ready');
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
  if (!recordedWav) return;
  if (!selectedModelReady()) {
    setStatus('Selected model is not downloaded.', 'error');
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
    audio_data: Array.from(recordedWav),
    model: modelSelect.value,
    language: languageSelect.value,
    save_markdown: saveMarkdownCheckbox.checked,
    output_mode: getOutputMode(),
    client: getSelectedCoachnotesClient(),
    diarization_mode: diarizationModeSelect.value,
  };

  try {
    const result = await invoke('transcribe_recording', { options });
    transcriptOutput.textContent = result.transcript || '';
    renderWarnings(result.warnings || []);
    resultSection.hidden = false;
    savedTranscriptPath = result.saved_path || null;

    if (savedTranscriptPath) {
      openFileBtn.hidden = false;
    } else {
      openFileBtn.hidden = true;
    }

    if (result.diarization_applied) {
      setStatus('Transcription complete (2-speaker mode)', 'ready');
    } else {
      setStatus('Transcription complete', 'ready');
    }
  } catch (error) {
    renderWarnings([]);
    setStatus(`Transcription failed: ${String(error)}`, 'error');
  } finally {
    isTranscribing = false;
    syncActionButtons();
  }
}

modelSelect.addEventListener('change', async () => {
  try {
    setupState = await invoke('set_selected_model', { model: modelSelect.value });
    recordedWav = null;
    renderSetupState();
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

captureModeSelect.addEventListener('change', () => {
  updateCaptureModeHelp();
});

diarizationModeSelect.addEventListener('change', () => {
  if (diarizationModeSelect.value === 'tdrz_2speaker') {
    setStatus(
      '2-speaker mode is experimental and English-only; requires small.en-tdrz model.',
      'idle'
    );
  }
});

startBtn.addEventListener('click', startRecording);
stopBtn.addEventListener('click', stopRecording);
transcribeBtn.addEventListener('click', transcribeRecording);

openFileBtn.addEventListener('click', async () => {
  if (!savedTranscriptPath) return;
  try {
    await invoke('show_in_folder', { path: savedTranscriptPath });
  } catch (error) {
    setStatus(`Could not open saved file: ${String(error)}`, 'error');
  }
});

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
  updateCaptureModeHelp();
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
