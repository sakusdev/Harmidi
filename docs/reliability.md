# Reliability pipeline

Harmidi intentionally keeps the transcription pipeline interpretable. Every note is supported by several independent signals instead of one FFT peak.

## Processing stages

1. **Band-limited resampling**
   - Windowed-sinc interpolation filters frequencies above the target Nyquist frequency.
   - This prevents high-frequency content from folding into false low notes during downsampling.
2. **Multi-resolution STFT**
   - Short windows improve attack timing.
   - Long windows improve low-frequency pitch resolution.
   - A note is stronger when multiple resolutions agree.
3. **HPSS soft masking**
   - Time-axis median energy estimates persistent harmonic content.
   - Frequency-axis median energy estimates broadband percussive content.
   - Pitch detection uses the harmonic mask while onset detection also uses the percussive residual.
4. **Log-frequency pitch evidence**
   - Candidate bins are aligned to MIDI-note frequencies rather than fixed linear FFT bins.
   - The detector evaluates the fundamental and up to twelve harmonics against a local spectral floor.
5. **Harmonic competition**
   - Every upper candidate is compared with all plausible lower fundamentals before note selection.
   - Independent octave doubling is preserved only when the upper note has separate direct and multi-resolution support.
6. **Residual extraction**
   - The strongest accepted note receives part of the explainable harmonic energy.
   - Its harmonic template is softly subtracted before the next note is selected.
   - Shared harmonics are only partially removed so real chords are not destroyed.
7. **Temporal confidence refinement**
   - Neighboring frames vote for stable pitches.
   - Isolated candidates without an onset receive a penalty.
8. **Viterbi-style note tracking**
   - Detector scores are calibrated into state probabilities.
   - The tracker balances attacks, continuity, gaps, releases, and re-articulation onsets.
9. **Note-level cleanup**
   - Adjacent segments are merged.
   - Long-overlap harmonic duplicates are removed.
   - Maximum polyphony is enforced by confidence over time.

## Quality modes

| Mode | Resolutions | Intended use |
| --- | --- | --- |
| Fast | Primary FFT only | Preview, slower mobile devices |
| Balanced | Short, primary, and long FFT | Default browser analysis |
| Accurate | 2048/4096/8192/16384 plus configured FFT | Offline-quality browser analysis |

Accurate mode uses more memory and CPU. All modes remain client-side and use a Web Worker.

## Confidence components

Each frame pitch and final note exposes:

- `confidence`: calibrated combined score
- `spectralConfidence`: strength relative to competing candidates
- `harmonicConfidence`: fraction of the expected harmonic series that is supported
- `multiresolutionConfidence`: agreement across FFT resolutions
- `temporalConfidence`: support from neighboring frames and the state tracker
- `independenceConfidence`: evidence that the pitch is not merely another note's harmonic
- `snrDb` on frame pitches: direct-component contrast above the estimated spectral floor

The UI displays the final note components as `S`, `H`, `T`, and `I`. A high combined score with a low independence score should be treated cautiously.

## Regression coverage

The Rust test suite currently covers:

- harmonic-rich single notes without octave/higher-harmonic duplication
- C-major polyphonic fundamentals
- short impulse rejection
- bounded confidence values
- one-frame temporal glitches
- sub-bin peak interpolation
- anti-aliasing during downsampling

These synthetic tests protect specific invariants. They do not replace evaluation on real recordings.

## Real-audio evaluation protocol

Store reference and prediction note lists as JSON arrays containing:

```json
{
  "midiNote": 60,
  "startSeconds": 0.5,
  "endSeconds": 1.2,
  "confidence": 0.84
}
```

Run:

```bash
npm run benchmark:notes -- reference.json prediction.json
```

The evaluator reports precision, recall, F1, onset error, and duration overlap. Recommended corpus groups:

- isolated vocals, whistle, bass, piano, and guitar
- major/minor/extended chords
- octave doubling and close intervals
- drums mixed with pitched instruments
- distortion, reverb, compression, and low-bitrate MP3
- quiet fundamentals with strong upper harmonics

Changes to detector thresholds should be accepted only when they improve the target corpus without materially regressing another group.

## Known limitations

- Dense mastered mixes can contain more overlapping sources than a deterministic browser DSP pipeline can separate reliably.
- Instruments with missing fundamentals can still produce octave or subharmonic ambiguity.
- Fast ornaments shorter than the configured minimum duration are intentionally removed.
- HPSS cannot identify instrument identity; source-separation models remain a future optional stage.
