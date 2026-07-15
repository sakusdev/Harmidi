# Harmidi

**音の中から、ノートを見つける。**

Harmidi is a local-first WebAssembly application that analyzes audio and converts overlapping pitched content into editable MIDI-note events. Audio never needs to leave the browser.

## Current milestone: polyphonic MVP

This first implementation includes:

- browser-side MP3/WAV/etc. decoding through the Web Audio API
- Rust analysis core compiled to WebAssembly
- STFT using `rustfft`
- harmonic-salience scoring across MIDI-note candidates
- lightweight temporal harmonic enhancement (an HPSS-inspired stage)
- simultaneous pitch selection with configurable maximum polyphony
- harmonic/octave suppression
- per-note temporal tracking with hysteresis and short-gap merging
- piano-roll visualization
- Standard MIDI File export
- analysis in a Web Worker so the UI stays responsive

This is an intentionally interpretable DSP baseline. It does **not** yet claim studio-grade transcription of dense mastered songs.

## Architecture

```text
Audio file
  ↓ Web Audio decodeAudioData
Mono Float32 PCM
  ↓ transferable buffer
Web Worker
  ↓
Rust / WebAssembly
  ├ resampling + normalization
  ├ STFT
  ├ temporal harmonic enhancement
  ├ multi-pitch harmonic salience
  ├ harmonic suppression
  └ temporal note tracking
  ↓
Detected note events
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
npm run dev
```

Production build:

```bash
npm run build
```

The WASM package is generated into `web/public/pkg` and copied by Vite into the final `dist` build.

## Cloudflare Workers deployment

Harmidi uses Workers Static Assets rather than Cloudflare Pages. The deployment configuration is in `wrangler.jsonc` and serves the Vite `dist` directory with SPA fallback enabled.

Authenticate once:

```bash
npx wrangler login
```

Run the production build locally through the Workers runtime:

```bash
npm run dev:worker
```

Validate the upload without deploying:

```bash
npm run deploy:dry-run
```

Deploy:

```bash
npm run deploy
```

For Cloudflare Git integration, use:

```text
Build command: npm run build
Deploy command: npx wrangler deploy
```

No server-side audio upload or storage binding is required for the current local-first MVP.

## Analysis controls

- **Maximum polyphony**: maximum note candidates retained in one analysis frame.
- **Sensitivity**: higher values reject weaker note candidates.
- **Pitch range**: limits candidate scoring and reduces false positives.
- **Minimum note duration**: rejects very short detections.
- **Harmonic enhancement**: favors frequency components that persist over neighboring frames and suppresses transient percussion.

## Roadmap

1. Improve onset-aware note splitting and velocity estimation.
2. Add true median-mask HPSS with separate harmonic/percussive previews.
3. Add CQT/chroma evidence and weighted estimator fusion.
4. Add optional ONNX/WebGPU two-stem and four-stem source separation.
5. Add stem-specialized transcription for vocals, bass, drums, and harmonic instruments.
6. Add editable piano-roll operations and beat-aware quantization.

## License

MIT
