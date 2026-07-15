import type { DetectedNote } from './types';

const NOTE_NAMES = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];

export function midiName(midi: number): string {
  const rounded = Math.round(midi);
  return `${NOTE_NAMES[((rounded % 12) + 12) % 12]}${Math.floor(rounded / 12) - 1}`;
}

export function drawPianoRoll(
  canvas: HTMLCanvasElement,
  notes: DetectedNote[],
  durationSeconds: number,
): void {
  const cssWidth = Math.max(640, canvas.clientWidth);
  const cssHeight = Math.max(280, canvas.clientHeight);
  const scale = window.devicePixelRatio || 1;
  canvas.width = Math.round(cssWidth * scale);
  canvas.height = Math.round(cssHeight * scale);

  const context = canvas.getContext('2d');
  if (context === null) return;
  context.scale(scale, scale);
  context.clearRect(0, 0, cssWidth, cssHeight);

  const minMidi = notes.length > 0 ? Math.max(0, Math.min(...notes.map((note) => note.midiNote)) - 2) : 48;
  const maxMidi = notes.length > 0 ? Math.min(127, Math.max(...notes.map((note) => note.midiNote)) + 2) : 72;
  const rows = Math.max(1, maxMidi - minMidi + 1);
  const rowHeight = cssHeight / rows;
  const safeDuration = Math.max(0.1, durationSeconds);

  context.font = '11px ui-monospace, monospace';
  context.textBaseline = 'middle';

  for (let midi = minMidi; midi <= maxMidi; midi += 1) {
    const y = cssHeight - (midi - minMidi + 1) * rowHeight;
    const isC = midi % 12 === 0;
    context.fillStyle = isC ? 'rgba(255,255,255,0.065)' : 'rgba(255,255,255,0.018)';
    context.fillRect(0, y, cssWidth, rowHeight);
    context.strokeStyle = 'rgba(255,255,255,0.06)';
    context.beginPath();
    context.moveTo(0, y);
    context.lineTo(cssWidth, y);
    context.stroke();

    if (isC) {
      context.fillStyle = 'rgba(255,255,255,0.5)';
      context.fillText(midiName(midi), 6, y + rowHeight / 2);
    }
  }

  for (const note of notes) {
    const x = (note.startSeconds / safeDuration) * cssWidth;
    const width = Math.max(2, ((note.endSeconds - note.startSeconds) / safeDuration) * cssWidth);
    const y = cssHeight - (note.midiNote - minMidi + 1) * rowHeight + 1;
    const alpha = 0.4 + note.confidence * 0.6;
    context.fillStyle = `rgba(115, 224, 255, ${alpha})`;
    context.fillRect(x, y, width, Math.max(2, rowHeight - 2));
  }
}
