export type AnalysisOptions = {
  fftSize: number;
  hopSize: number;
  minMidi: number;
  maxMidi: number;
  maxPolyphony: number;
  sensitivity: number;
  minNoteMs: number;
  targetSampleRate: number;
  harmonicEnhancement: boolean;
};

export type FramePitch = {
  midiNote: number;
  frequencyHz: number;
  confidence: number;
  centsOffset: number;
};

export type FrameSummary = {
  timeSeconds: number;
  rms: number;
  onsetStrength: number;
  pitches: FramePitch[];
};

export type DetectedNote = {
  midiNote: number;
  startSeconds: number;
  endSeconds: number;
  velocity: number;
  confidence: number;
  centsOffset: number;
};

export type AnalysisDiagnostics = {
  inputSamples: number;
  analyzedSamples: number;
  analyzedSampleRate: number;
  frameCount: number;
  fftSize: number;
  hopSize: number;
  harmonicEnhancement: boolean;
};

export type AnalysisResult = {
  durationSeconds: number;
  notes: DetectedNote[];
  frames: FrameSummary[];
  diagnostics: AnalysisDiagnostics;
};

export type WorkerRequest = {
  type: 'analyze';
  samples: Float32Array;
  sampleRate: number;
  options: AnalysisOptions;
};

export type WorkerResponse =
  | { type: 'ready' }
  | { type: 'result'; result: AnalysisResult }
  | { type: 'error'; message: string };
