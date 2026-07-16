# Transient rejection

Harmidi applies three independent safeguards before treating broadband attacks as pitched notes:

1. HPSS separates persistent horizontal spectral structure from frequency-wide percussive structure.
2. Spectral tonality rejects frames whose geometric-to-arithmetic spectral mean indicates near-flat broadband energy.
3. Time-domain crest factor rejects strong-onset frames whose peak amplitude is extreme relative to RMS.

The crest gate is applied only when both the crest factor and normalized onset strength are high. This avoids rejecting ordinary sustained tones and most pitched attacks while preventing an impulse from becoming a false 100–200 ms note merely because overlapping FFT windows contain it repeatedly.

Synthetic regression tests cover flat-spectrum tonality, impulse-versus-tone crest factor, and repeated short impulse rejection.
