import './style.css';
import { decodeAudioFile } from './audio';
import { createMidiFile } from './midi';
import { drawPianoRoll, midiName } from './piano-roll';
import type {
  AnalysisOptions,
  AnalysisQuality,
  AnalysisResult,
  WorkerResponse,
} from './types';

const app = document.querySelector<HTMLElement>('#app');
if (app === null) throw new Error('Application root was not found.');

app.innerHTML = `
  <section class="hero">
    <div>
      <p class="eyebrow">LOCAL · RUST · WEBASSEMBLY</p>
      <h1>Harmidi</h1>
      <p class="lead">音声の中から、重なったノートを根拠付きで見つける。</p>
    </div>
    <span class="status-pill" id="engine-status">WASM待機中</span>
  </section>

  <section class="panel upload-panel">
    <label class="drop-zone" id="drop-zone">
      <input id="file-input" type="file" accept="audio/*,.mp3,.wav,.flac,.m4a,.ogg" />
      <span class="drop-title">音声ファイルをドロップ</span>
      <span class="drop-subtitle">MP3 / WAV / FLAC / M4A / OGG · 最大3分を解析</span>
    </label>
    <audio id="audio-player" controls></audio>
  </section>

  <section class="workspace">
    <aside class="panel controls">
      <h2>解析設定</h2>
      <label>音源プリセット
        <select id="preset">
          <option value="general" selected>一般楽曲</option>
          <option value="vocal">ボーカル / 単旋律</option>
          <option value="bass">ベース</option>
          <option value="piano">ピアノ / 鍵盤</option>
          <option value="guitar">ギター</option>
          <option value="dense">高密度ミックス</option>
        </select>
      </label>
      <label>解析品質
        <select id="quality">
          <option value="fast">高速</option>
          <option value="balanced" selected>標準</option>
          <option value="accurate">高精度</option>
        </select>
      </label>
      <label>最大同時発音数 <output id="polyphony-value">6</output>
        <input id="polyphony" type="range" min="1" max="12" value="6" />
      </label>
      <label>感度 <output id="sensitivity-value">58%</output>
        <input id="sensitivity" type="range" min="20" max="90" value="58" />
      </label>
      <div class="two-column">
        <label>最低音
          <input id="min-midi" type="number" min="0" max="126" value="36" />
        </label>
        <label>最高音
          <input id="max-midi" type="number" min="1" max="127" value="96" />
        </label>
      </div>
      <label>最短ノート長（ms）
        <input id="min-note" type="number" min="30" max="1000" step="10" value="90" />
      </label>
      <fieldset class="feature-options">
        <legend>信頼性処理</legend>
        <label class="check-row">
          <input id="hpss" type="checkbox" checked />
          <span>調波・打楽器分離（HPSS）</span>
        </label>
        <label class="check-row">
          <input id="multiresolution" type="checkbox" checked />
          <span>マルチ解像度STFT</span>
        </label>
        <label class="check-row">
          <input id="residual" type="checkbox" checked />
          <span>残差反復による多音抽出</span>
        </label>
        <label class="check-row">
          <input id="temporal" type="checkbox" checked />
          <span>時間方向の状態追跡</span>
        </label>
      </fieldset>
      <button id="analyze" class="primary" disabled>高信頼度解析を開始</button>
      <button id="export" disabled>MIDIを書き出す</button>
      <label>BPM（MIDI用）
        <input id="bpm" type="number" min="20" max="300" value="120" />
      </label>
    </aside>

    <section class="panel results">
      <div class="result-header">
        <div>
          <h2>ピアノロール</h2>
          <p id="summary">音声を読み込むと解析結果が表示されます。</p>
        </div>
        <div class="spinner" id="spinner" hidden></div>
      </div>
      <div class="diagnostics" id="diagnostics" hidden></div>
      <canvas id="piano-roll"></canvas>
      <div class="note-list" id="note-list"></div>
    </section>
  </section>
`;

function requireElement<T extends HTMLElement>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (element === null) throw new Error(`Missing element: ${selector}`);
  return element;
}

const fileInput = requireElement<HTMLInputElement>('#file-input');
const dropZone = requireElement<HTMLLabelElement>('#drop-zone');
const player = requireElement<HTMLAudioElement>('#audio-player');
const analyzeButton = requireElement<HTMLButtonElement>('#analyze');
const exportButton = requireElement<HTMLButtonElement>('#export');
const engineStatus = requireElement<HTMLSpanElement>('#engine-status');
const summary = requireElement<HTMLParagraphElement>('#summary');
const diagnostics = requireElement<HTMLDivElement>('#diagnostics');
const spinner = requireElement<HTMLDivElement>('#spinner');
const canvas = requireElement<HTMLCanvasElement>('#piano-roll');
const noteList = requireElement<HTMLDivElement>('#note-list');
const preset = requireElement<HTMLSelectElement>('#preset');
const polyphony = requireElement<HTMLInputElement>('#polyphony');
const polyphonyValue = requireElement<HTMLOutputElement>('#polyphony-value');
const sensitivity = requireElement<HTMLInputElement>('#sensitivity');
const sensitivityValue = requireElement<HTMLOutputElement>('#sensitivity-value');

let currentSamples: Float32Array | null = null;
let currentSampleRate = 0;
let currentFileName = 'harmidi';
let currentResult: AnalysisResult | null = null;
let objectUrl: string | null = null;

const worker = new Worker(new URL('./analysis.worker.ts', import.meta.url), { type: 'module' });
worker.addEventListener('message', (event: MessageEvent<WorkerResponse>) => {
  if (event.data.type === 'ready') {
    engineStatus.textContent = 'WASM Worker準備完了';
    return;
  }

  spinner.hidden = true;
  analyzeButton.disabled = currentSamples === null;

  if (event.data.type === 'error') {
    engineStatus.textContent = '解析エラー';
    summary.textContent = event.data.message;
    diagnostics.hidden = true;
    return;
  }

  currentResult = event.data.result;
  exportButton.disabled = currentResult.notes.length === 0;
  engineStatus.textContent = '解析完了';
  const quality = qualityLabel(currentResult.diagnostics.quality);
  summary.textContent = `${currentResult.notes.length}ノート · ${currentResult.durationSeconds.toFixed(1)}秒 · ${quality} · ${currentResult.diagnostics.frameCount}フレーム`;
  renderResult(currentResult);
});

polyphony.addEventListener('input', () => {
  polyphonyValue.value = polyphony.value;
});
sensitivity.addEventListener('input', () => {
  sensitivityValue.value = `${sensitivity.value}%`;
});

async function loadFile(file: File): Promise<void> {
  engineStatus.textContent = '音声をデコード中';
  summary.textContent = 'ブラウザ内でPCMへ変換しています…';
  diagnostics.hidden = true;
  currentResult = null;
  exportButton.disabled = true;

  if (objectUrl !== null) URL.revokeObjectURL(objectUrl);
  objectUrl = URL.createObjectURL(file);
  player.src = objectUrl;
  currentFileName = file.name.replace(/\.[^.]+$/, '') || 'harmidi';

  try {
    const decoded = await decodeAudioFile(file);
    currentSamples = decoded.samples;
    currentSampleRate = decoded.sampleRate;
    analyzeButton.disabled = false;
    engineStatus.textContent = '解析可能';
    summary.textContent = `${decoded.durationSeconds.toFixed(1)}秒 · ${decoded.sampleRate.toLocaleString()} Hz${decoded.wasTrimmed ? ' · 3分で切り詰め' : ''}`;
  } catch (error) {
    currentSamples = null;
    analyzeButton.disabled = true;
    engineStatus.textContent = '読込エラー';
    summary.textContent = error instanceof Error ? error.message : String(error);
  }
}

fileInput.addEventListener('change', () => {
  const file = fileInput.files?.[0];
  if (file !== undefined) void loadFile(file);
});

for (const eventName of ['dragenter', 'dragover'] as const) {
  dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    dropZone.classList.add('dragging');
  });
}
for (const eventName of ['dragleave', 'drop'] as const) {
  dropZone.addEventListener(eventName, (event) => {
    event.preventDefault();
    dropZone.classList.remove('dragging');
  });
}
dropZone.addEventListener('drop', (event) => {
  const file = event.dataTransfer?.files[0];
  if (file !== undefined) void loadFile(file);
});

analyzeButton.addEventListener('click', () => {
  if (currentSamples === null) return;

  const minMidi = Number(requireElement<HTMLInputElement>('#min-midi').value);
  const maxMidi = Number(requireElement<HTMLInputElement>('#max-midi').value);
  const quality = requireElement<HTMLSelectElement>('#quality').value as AnalysisQuality;
  const options: AnalysisOptions = {
    fftSize: 4096,
    hopSize: quality === 'accurate' ? 256 : 512,
    minMidi: Math.min(minMidi, maxMidi - 1),
    maxMidi: Math.max(maxMidi, minMidi + 1),
    maxPolyphony: Number(polyphony.value),
    sensitivity: Number(sensitivity.value) / 100,
    minNoteMs: Number(requireElement<HTMLInputElement>('#min-note').value),
    targetSampleRate: quality === 'accurate' ? 32_000 : 22_050,
    harmonicEnhancement: requireElement<HTMLInputElement>('#hpss').checked,
    quality,
    useHpss: requireElement<HTMLInputElement>('#hpss').checked,
    useMultiresolution: requireElement<HTMLInputElement>('#multiresolution').checked,
    useResidual: requireElement<HTMLInputElement>('#residual').checked,
    useTemporalTracking: requireElement<HTMLInputElement>('#temporal').checked,
  };

  const samples = currentSamples.slice();
  spinner.hidden = false;
  diagnostics.hidden = true;
  analyzeButton.disabled = true;
  exportButton.disabled = true;
  engineStatus.textContent = `${qualityLabel(quality)}で解析中`;
  summary.textContent = 'HPSS・マルチ解像度STFT・残差抽出・時間追跡を実行しています…';
  worker.postMessage(
    { type: 'analyze', samples, sampleRate: currentSampleRate, options },
    [samples.buffer],
  );
});

exportButton.addEventListener('click', () => {
  if (currentResult === null) return;
  const bpm = Number(requireElement<HTMLInputElement>('#bpm').value);
  const bytes = createMidiFile(currentResult.notes, bpm);
  const midiBuffer = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;
  const blob = new Blob([midiBuffer], { type: 'audio/midi' });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = `${currentFileName}.mid`;
  anchor.click();
  URL.revokeObjectURL(url);
});

function qualityLabel(quality: AnalysisQuality): string {
  switch (quality) {
    case 'fast': return '高速';
    case 'accurate': return '高精度';
    default: return '標準';
  }
}

function percent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function renderResult(result: AnalysisResult): void {
  drawPianoRoll(canvas, result.notes, result.durationSeconds);
  const diagnosticItems = [
    `FFT ${result.diagnostics.resolutionFftSizes.join(' / ')}`,
    result.diagnostics.hpssEnabled ? 'HPSS ON' : 'HPSS OFF',
    result.diagnostics.residualExtractionEnabled ? '残差抽出 ON' : '残差抽出 OFF',
    result.diagnostics.temporalTrackingEnabled ? '時間追跡 ON' : '時間追跡 OFF',
    `Resampler ${result.diagnostics.resampler}`,
  ];
  diagnostics.innerHTML = diagnosticItems.map((item) => `<span>${item}</span>`).join('');
  diagnostics.hidden = false;

  const visibleNotes = result.notes.slice(0, 120);
  noteList.innerHTML = visibleNotes
    .map(
      (note) => `
        <div class="note-row">
          <strong>${midiName(note.midiNote)}</strong>
          <span>${note.startSeconds.toFixed(2)}–${note.endSeconds.toFixed(2)} s</span>
          <span class="confidence-total">${percent(note.confidence)}</span>
          <span>${note.centsOffset >= 0 ? '+' : ''}${note.centsOffset.toFixed(1)} cent</span>
          <span class="confidence-breakdown">
            S ${percent(note.spectralConfidence)} · H ${percent(note.harmonicConfidence)} · T ${percent(note.temporalConfidence)} · I ${percent(note.independenceConfidence)}
          </span>
        </div>
      `,
    )
    .join('');

  if (result.notes.length > visibleNotes.length) {
    noteList.insertAdjacentHTML('beforeend', `<p class="muted">ほか ${result.notes.length - visibleNotes.length} ノート</p>`);
  }
}

window.addEventListener('resize', () => {
  if (currentResult !== null) drawPianoRoll(canvas, currentResult.notes, currentResult.durationSeconds);
});
