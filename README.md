# Harmidi

**音の中から、ノートを見つける。**

Harmidi is a local-first WebAssembly application that analyzes audio and converts overlapping pitched content into editable MIDI-note events. Audio never needs to leave the browser.

## Current milestone: interpretable polyphonic transcription

The browser pipeline includes:

- browser-side MP3/WAV/etc. decoding through the Web Audio API
- Rust analysis core compiled to WebAssembly
- anti-aliased windowed-sinc resampling
- multi-resolution STFT using `rustfft`
- harmonic/percussive soft-mask separation (HPSS)
- MIDI-aligned log-frequency pitch evidence
- fundamental and harmonic-series scoring
- harmonic competition and octave false-positive suppression
- iterative residual harmonic-template extraction
- sub-bin frequency interpolation
- calibrated confidence components for every detected note
- onset-aware Viterbi-style temporal note tracking
- note-level harmonic duplicate removal and polyphony enforcement
- instrument-aware vocal, bass, piano, guitar, and dense-mix presets
- piano-roll visualization and Standard MIDI File export
- analysis in a Web Worker so the UI stays responsive

This remains an interpretable DSP system rather than a claim of perfect studio-grade transcription. Dense mastered mixes, missing fundamentals, heavy distortion, and strong source overlap remain difficult cases.

## Architecture

```text
Audio file
  ↓ Web Audio decodeAudioData
Mono Float32 PCM
  ↓ transferable buffer
Web Worker
  ↓
Rust / WebAssembly
  ├ band-limited resampling + normalization
  ├ multi-resolution STFT
  ├ HPSS harmonic/percussive masks
  ├ MIDI-aligned pitch and harmonic evidence
  ├ cross-resolution confidence fusion
  ├ harmonic competition
  ├ residual iterative note extraction
  ├ temporal confidence refinement
  ├ Viterbi note-state tracking
  └ note-level cleanup / polyphony limiting
  ↓
Detected note events + confidence evidence
  ├ piano-roll UI
  └ MIDI file writer
```

The production build is deployed as Cloudflare Workers Static Assets. Audio decoding, analysis, and MIDI generation remain entirely in the browser; Cloudflare only serves the compiled HTML, JavaScript, CSS, and WebAssembly files.

## Development

Requirements:

- Node.js 22.12 or newer
- Rust stable
- `wasm32-unknown-unknown` target
- `wasm-pack`

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack --locked
npm install
npm run dev:full
```

Node-only production build using the committed WASM package:

```bash
npm run build
```

Full build after changing Rust:

```bash
npm run build:full
```

The WASM package is generated into `web/public/pkg` and copied by Vite into the final `dist` build. GitHub Actions verifies that the committed package matches the Rust source.

## Cloudflare Workers deployment

Harmidi uses Workers Static Assets rather than Cloudflare Pages. The deployment configuration is in `wrangler.jsonc` and serves the Vite `dist` directory with SPA fallback enabled.

```bash
npx wrangler login
npm run dev:worker
npm run deploy:dry-run
npm run deploy
```

For Cloudflare Git integration:

```text
Build command: npm run build
Deploy command: npx wrangler deploy
```

No server-side audio upload or storage binding is required.

## Analysis controls

- **Source preset**: adjusts quality, pitch range, polyphony, sensitivity, duration, and reliability stages for general music, vocals, bass, piano, guitar, or dense mixes.
- **Quality**: Fast uses one FFT resolution; Balanced combines short/primary/long windows; Accurate combines up to 2048/4096/8192/16384-point analyses.
- **Maximum polyphony**: maximum simultaneous notes retained after temporal cleanup.
- **Sensitivity**: higher values reject weaker evidence.
- **Pitch range**: limits candidate scoring and reduces impossible detections.
- **Minimum note duration**: rejects very short events.
- **HPSS**: separates persistent harmonic energy from broadband transients.
- **Multi-resolution STFT**: requires agreement across time/frequency resolutions.
- **Residual extraction**: softly subtracts explained harmonics before selecting another note.
- **Temporal tracking**: optimizes note states across frames instead of treating frames independently.

The result list displays total confidence and its main evidence components:

- `S`: spectral evidence
- `H`: harmonic-series evidence
- `T`: temporal evidence
- `I`: independence from another note's harmonic series

See [docs/reliability.md](docs/reliability.md) for implementation details and evaluation guidance.

## Reliability evaluation

Compare reference and predicted note JSON files with:

```bash
npm run benchmark:notes -- reference.json prediction.json
```

The evaluator reports precision, recall, F1, mean onset error, and mean duration intersection-over-union. Detector threshold changes should be evaluated against a varied real-audio corpus rather than accepted from one example.

## Roadmap

1. Add optional source separation with WebGPU/ONNX and original-vs-stem consensus.
2. Add beat-aware quantization and editable piano-roll operations.
3. Grow the public real-audio benchmark corpus and publish per-category metrics.

## License

MIT
