use rustfft::{FftPlanner, num_complex::Complex32};
use serde::{Deserialize, Serialize};

use crate::dsp::{
    frequency_to_midi_float, hann_window, linear_resample, local_peak, median,
    midi_to_frequency, remove_dc_and_normalize, rms,
};
use crate::tracking::track_notes;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisOptions {
    pub fft_size: usize,
    pub hop_size: usize,
    pub min_midi: u8,
    pub max_midi: u8,
    pub max_polyphony: usize,
    pub sensitivity: f32,
    pub min_note_ms: f32,
    pub target_sample_rate: u32,
    pub harmonic_enhancement: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePitch {
    pub midi_note: u8,
    pub frequency_hz: f32,
    pub confidence: f32,
    pub cents_offset: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSummary {
    pub time_seconds: f32,
    pub rms: f32,
    pub onset_strength: f32,
    pub pitches: Vec<FramePitch>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedNote {
    pub midi_note: u8,
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub velocity: u8,
    pub confidence: f32,
    pub cents_offset: f32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisDiagnostics {
    pub input_samples: usize,
    pub analyzed_samples: usize,
    pub analyzed_sample_rate: u32,
    pub frame_count: usize,
    pub fft_size: usize,
    pub hop_size: usize,
    pub harmonic_enhancement: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub duration_seconds: f32,
    pub notes: Vec<DetectedNote>,
    pub frames: Vec<FrameSummary>,
    pub diagnostics: AnalysisDiagnostics,
}

pub fn analyze(
    input_samples: &[f32],
    input_sample_rate: u32,
    options: &AnalysisOptions,
) -> Result<AnalysisResult, String> {
    validate_options(options)?;

    let analyzed_rate = options.target_sample_rate.min(input_sample_rate).max(8_000);
    let mut samples = linear_resample(input_samples, input_sample_rate, analyzed_rate);
    remove_dc_and_normalize(&mut samples);

    if samples.len() < options.fft_size {
        samples.resize(options.fft_size, 0.0);
    }

    let bin_count = options.fft_size / 2 + 1;
    let frame_count = 1 + (samples.len() - options.fft_size) / options.hop_size;
    let mut spectra = vec![0.0_f32; frame_count * bin_count];
    let mut frame_rms = vec![0.0_f32; frame_count];
    let window = hann_window(options.fft_size);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(options.fft_size);
    let mut buffer = vec![Complex32::new(0.0, 0.0); options.fft_size];

    for frame_index in 0..frame_count {
        let start = frame_index * options.hop_size;
        let frame = &samples[start..start + options.fft_size];
        frame_rms[frame_index] = rms(frame);

        for (index, target) in buffer.iter_mut().enumerate() {
            *target = Complex32::new(frame[index] * window[index], 0.0);
        }
        fft.process(&mut buffer);

        let spectrum = &mut spectra[frame_index * bin_count..(frame_index + 1) * bin_count];
        for (bin, value) in spectrum.iter_mut().enumerate() {
            let magnitude = buffer[bin].norm() / options.fft_size as f32;
            *value = (1.0 + magnitude * 120.0).ln();
        }
    }

    if options.harmonic_enhancement {
        apply_temporal_harmonic_enhancement(&mut spectra, frame_count, bin_count);
    }

    let mut frames = Vec::with_capacity(frame_count);
    let mut previous_spectrum = vec![0.0_f32; bin_count];
    let hop_seconds = options.hop_size as f32 / analyzed_rate as f32;
    let global_rms = frame_rms.iter().copied().sum::<f32>() / frame_rms.len().max(1) as f32;
    let silence_floor = (global_rms * 0.12).max(0.0015);

    for frame_index in 0..frame_count {
        let spectrum = &spectra[frame_index * bin_count..(frame_index + 1) * bin_count];
        let onset_strength = spectral_flux(spectrum, &previous_spectrum);
        previous_spectrum.copy_from_slice(spectrum);

        let pitches = if frame_rms[frame_index] >= silence_floor {
            detect_frame_pitches(spectrum, analyzed_rate, options)
        } else {
            Vec::new()
        };

        frames.push(FrameSummary {
            time_seconds: frame_index as f32 * hop_seconds,
            rms: frame_rms[frame_index],
            onset_strength,
            pitches,
        });
    }

    let notes = track_notes(
        &frames,
        options.min_midi,
        options.max_midi,
        hop_seconds,
        options.min_note_ms,
    );

    Ok(AnalysisResult {
        duration_seconds: input_samples.len() as f32 / input_sample_rate as f32,
        notes,
        frames,
        diagnostics: AnalysisDiagnostics {
            input_samples: input_samples.len(),
            analyzed_samples: samples.len(),
            analyzed_sample_rate: analyzed_rate,
            frame_count,
            fft_size: options.fft_size,
            hop_size: options.hop_size,
            harmonic_enhancement: options.harmonic_enhancement,
        },
    })
}

fn validate_options(options: &AnalysisOptions) -> Result<(), String> {
    if !options.fft_size.is_power_of_two() || !(512..=16_384).contains(&options.fft_size) {
        return Err("fftSize must be a power of two between 512 and 16384".into());
    }
    if options.hop_size == 0 || options.hop_size > options.fft_size {
        return Err("hopSize must be between 1 and fftSize".into());
    }
    if options.min_midi >= options.max_midi {
        return Err("minMidi must be lower than maxMidi".into());
    }
    if options.max_polyphony == 0 || options.max_polyphony > 32 {
        return Err("maxPolyphony must be between 1 and 32".into());
    }
    if !(0.0..=1.0).contains(&options.sensitivity) {
        return Err("sensitivity must be between 0 and 1".into());
    }
    Ok(())
}

fn apply_temporal_harmonic_enhancement(
    spectra: &mut [f32],
    frame_count: usize,
    bin_count: usize,
) {
    let original = spectra.to_vec();
    let radius = 3_usize;
    let mut neighborhood = Vec::with_capacity(radius * 2 + 1);

    for frame in 0..frame_count {
        let start_frame = frame.saturating_sub(radius);
        let end_frame = (frame + radius).min(frame_count - 1);
        for bin in 0..bin_count {
            neighborhood.clear();
            for neighbor_frame in start_frame..=end_frame {
                neighborhood.push(original[neighbor_frame * bin_count + bin]);
            }
            let persistent = median(&mut neighborhood);
            let current = original[frame * bin_count + bin];
            spectra[frame * bin_count + bin] = current * 0.58 + persistent * 0.42;
        }
    }
}

#[derive(Debug, Clone)]
struct PitchEvidence {
    midi: u8,
    score: f32,
    direct: f32,
    harmonics: Vec<f32>,
    normalized: f32,
    adjusted: f32,
}

fn detect_frame_pitches(
    spectrum: &[f32],
    sample_rate: u32,
    options: &AnalysisOptions,
) -> Vec<FramePitch> {
    let midi_count = options.max_midi as usize - options.min_midi as usize + 1;
    let nyquist = sample_rate as f32 / 2.0;
    let bin_hz = sample_rate as f32 / options.fft_size as f32;
    let mut evidence = Vec::with_capacity(midi_count);

    for midi in options.min_midi..=options.max_midi {
        let fundamental = midi_to_frequency(midi);
        let mut harmonics = Vec::with_capacity(10);
        let mut weighted_sum = 0.0_f32;
        let mut weight_total = 0.0_f32;
        let mut coverage = 0_usize;

        for harmonic in 1..=10 {
            let frequency = fundamental * harmonic as f32;
            if frequency >= nyquist {
                break;
            }
            let center_bin = (frequency / bin_hz).round() as usize;
            let contrast = spectral_contrast(spectrum, center_bin);
            let weight = if harmonic == 1 {
                1.75
            } else {
                1.0 / (harmonic as f32).powf(0.72)
            };
            harmonics.push(contrast);
            weighted_sum += contrast * weight;
            weight_total += weight;
            if contrast > 0.08 {
                coverage += 1;
            }
        }

        let direct = harmonics.first().copied().unwrap_or(0.0);
        let score = if weight_total > 0.0 {
            direct * 1.25
                + weighted_sum / weight_total.powf(0.58)
                + coverage as f32 * 0.035
        } else {
            0.0
        };

        evidence.push(PitchEvidence {
            midi,
            score,
            direct,
            harmonics,
            normalized: 0.0,
            adjusted: 0.0,
        });
    }

    let maximum = evidence
        .iter()
        .map(|candidate| candidate.score)
        .fold(0.0_f32, f32::max);
    if maximum <= 1.0e-6 {
        return Vec::new();
    }

    let threshold_ratio = 0.17 + options.sensitivity * 0.38;
    let mut candidates = Vec::new();
    for (index, candidate) in evidence.iter().enumerate() {
        let left = index
            .checked_sub(1)
            .map_or(0.0, |value| evidence[value].score);
        let right = evidence.get(index + 1).map_or(0.0, |value| value.score);
        let normalized = candidate.score / maximum;

        // Strict on the lower neighbor collapses low-frequency plateaus where
        // adjacent MIDI notes resolve to the same FFT bin.
        if candidate.score > left
            && candidate.score >= right
            && normalized >= threshold_ratio
            && candidate.direct > 1.0e-5
        {
            let mut candidate = candidate.clone();
            candidate.normalized = normalized;
            candidate.adjusted = normalized;
            candidates.push(candidate);
        }
    }

    // Evaluate every candidate against every plausible lower fundamental before
    // sorting. The previous implementation only compared against already selected
    // notes, so a loud octave harmonic could be selected first and escape removal.
    let snapshot = candidates.clone();
    for candidate in &mut candidates {
        let mut penalty = 1.0_f32;

        for lower in &snapshot {
            if lower.midi >= candidate.midi {
                continue;
            }
            let Some(order) = harmonic_order(lower.midi, candidate.midi) else {
                continue;
            };
            if lower.normalized < candidate.normalized * 0.42 {
                continue;
            }
            if has_independent_upper_support(candidate, lower, order) {
                continue;
            }

            let dominance = ((lower.normalized / candidate.normalized.max(1.0e-6) - 0.35)
                / 0.75)
                .clamp(0.0, 1.0);
            let maximum_suppression = if order <= 4 { 0.86 } else { 0.92 };
            penalty *= (1.0 - maximum_suppression * dominance).clamp(0.04, 1.0);
        }

        candidate.adjusted = candidate.normalized * penalty;
    }

    candidates.sort_by(|left, right| {
        right
            .adjusted
            .total_cmp(&left.adjusted)
            .then(left.midi.cmp(&right.midi))
    });

    let mut selected: Vec<PitchEvidence> = Vec::with_capacity(options.max_polyphony);
    for candidate in candidates {
        if candidate.adjusted < threshold_ratio {
            continue;
        }
        if selected
            .iter()
            .any(|existing| existing.midi.abs_diff(candidate.midi) <= 1)
        {
            continue;
        }

        selected.push(candidate);
        if selected.len() >= options.max_polyphony {
            break;
        }
    }

    selected
        .into_iter()
        .map(|candidate| {
            let expected_frequency = midi_to_frequency(candidate.midi);
            let center_bin = (expected_frequency / bin_hz).round() as usize;
            let (peak_bin, _) = local_peak(spectrum, center_bin, 2);
            let measured_frequency = peak_bin as f32 * bin_hz;
            let measured_midi = if measured_frequency > 0.0 {
                frequency_to_midi_float(measured_frequency)
            } else {
                candidate.midi as f32
            };

            FramePitch {
                midi_note: candidate.midi,
                frequency_hz: measured_frequency,
                confidence: candidate.adjusted.clamp(0.0, 1.0),
                cents_offset: ((measured_midi - candidate.midi as f32) * 100.0)
                    .clamp(-100.0, 100.0),
            }
        })
        .collect()
}

fn spectral_contrast(spectrum: &[f32], center_bin: usize) -> f32 {
    if spectrum.is_empty() {
        return 0.0;
    }

    let (_, peak) = local_peak(spectrum, center_bin, 1);
    let start = center_bin.saturating_sub(8);
    let end = (center_bin + 8).min(spectrum.len() - 1);
    let mut floor_samples = Vec::with_capacity(12);

    for (bin, value) in spectrum.iter().enumerate().take(end + 1).skip(start) {
        if bin.abs_diff(center_bin) > 2 {
            floor_samples.push(*value);
        }
    }

    let floor = median(&mut floor_samples);
    (peak - floor).max(0.0)
}

fn harmonic_order(lower_midi: u8, upper_midi: u8) -> Option<usize> {
    let interval = upper_midi as f32 - lower_midi as f32;
    let mut best: Option<(usize, f32)> = None;

    for order in 2..=10 {
        let expected_interval = 12.0 * (order as f32).log2();
        let error = (interval - expected_interval).abs();
        if error <= 0.5 && best.is_none_or(|(_, best_error)| error < best_error) {
            best = Some((order, error));
        }
    }

    best.map(|(order, _)| order)
}

fn has_independent_upper_support(
    upper: &PitchEvidence,
    lower: &PitchEvidence,
    harmonic_order: usize,
) -> bool {
    let harmonic_value = lower
        .harmonics
        .get(harmonic_order - 1)
        .copied()
        .unwrap_or(upper.direct);
    let mut neighbors = Vec::with_capacity(2);

    if harmonic_order >= 2
        && let Some(value) = lower.harmonics.get(harmonic_order - 2)
    {
        neighbors.push(*value);
    }
    if let Some(value) = lower.harmonics.get(harmonic_order) {
        neighbors.push(*value);
    }

    let local_envelope = median(&mut neighbors);
    let decay_envelope = lower.direct / (harmonic_order as f32).powf(0.85) * 0.45;
    let expected = local_envelope.max(decay_envelope).max(0.05);
    let excess = harmonic_value / expected;
    let score_ratio = upper.score / lower.score.max(1.0e-6);
    let direct_ratio = upper.direct / lower.direct.max(1.0e-6);

    if harmonic_order == 2 {
        // Preserve a genuinely doubled octave only when the upper fundamental
        // rises above the lower note's normal second-harmonic envelope.
        excess >= 1.10 && score_ratio >= 0.92 && direct_ratio >= 0.95
    } else {
        excess >= 1.35 && score_ratio >= 1.03 && direct_ratio >= 0.78
    }
}

fn spectral_flux(current: &[f32], previous: &[f32]) -> f32 {
    if current.is_empty() || previous.len() != current.len() {
        return 0.0;
    }

    let positive_change = current
        .iter()
        .zip(previous)
        .map(|(now, before)| (now - before).max(0.0))
        .sum::<f32>();
    (positive_change / current.len() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_options() -> AnalysisOptions {
        AnalysisOptions {
            fft_size: 4096,
            hop_size: 512,
            min_midi: 36,
            max_midi: 96,
            max_polyphony: 6,
            sensitivity: 0.5,
            min_note_ms: 80.0,
            target_sample_rate: 22_050,
            harmonic_enhancement: true,
        }
    }

    fn add_harmonic_series(
        spectrum: &mut [f32],
        midi: u8,
        amplitude: f32,
        sample_rate: u32,
        fft_size: usize,
    ) {
        let bin_hz = sample_rate as f32 / fft_size as f32;
        let fundamental = midi_to_frequency(midi);

        for harmonic in 1..=10 {
            let frequency = fundamental * harmonic as f32;
            if frequency >= sample_rate as f32 / 2.0 {
                break;
            }
            let center = (frequency / bin_hz).round() as usize;
            let value = amplitude / (harmonic as f32).powf(0.72);
            spectrum[center] += value;
            if center > 0 {
                spectrum[center - 1] += value * 0.25;
            }
            if center + 1 < spectrum.len() {
                spectrum[center + 1] += value * 0.25;
            }
        }
    }

    #[test]
    fn rejects_non_power_of_two_fft_size() {
        let mut options = default_options();
        options.fft_size = 1000;
        assert!(validate_options(&options).is_err());
    }

    #[test]
    fn suppresses_octave_and_higher_harmonic_false_positives() {
        let options = default_options();
        let mut spectrum = vec![0.0; options.fft_size / 2 + 1];
        add_harmonic_series(
            &mut spectrum,
            48,
            1.0,
            options.target_sample_rate,
            options.fft_size,
        );

        let pitches = detect_frame_pitches(&spectrum, options.target_sample_rate, &options);
        assert!(pitches.iter().any(|pitch| pitch.midi_note == 48));
        assert!(!pitches.iter().any(|pitch| pitch.midi_note == 60));
        assert!(!pitches.iter().any(|pitch| pitch.midi_note == 67));
    }

    #[test]
    fn preserves_polyphonic_chord_fundamentals() {
        let options = default_options();
        let mut spectrum = vec![0.0; options.fft_size / 2 + 1];
        for (midi, amplitude) in [(60, 1.0), (64, 0.85), (67, 0.8)] {
            add_harmonic_series(
                &mut spectrum,
                midi,
                amplitude,
                options.target_sample_rate,
                options.fft_size,
            );
        }

        let pitches = detect_frame_pitches(&spectrum, options.target_sample_rate, &options);
        for midi in [60, 64, 67] {
            assert!(pitches.iter().any(|pitch| pitch.midi_note == midi));
        }
        assert!(!pitches.iter().any(|pitch| pitch.midi_note == 72));
    }

    #[test]
    fn preserves_strong_independent_octave_doubling() {
        let options = default_options();
        let mut spectrum = vec![0.0; options.fft_size / 2 + 1];
        add_harmonic_series(
            &mut spectrum,
            48,
            1.0,
            options.target_sample_rate,
            options.fft_size,
        );
        add_harmonic_series(
            &mut spectrum,
            60,
            1.4,
            options.target_sample_rate,
            options.fft_size,
        );

        let pitches = detect_frame_pitches(&spectrum, options.target_sample_rate, &options);
        assert!(pitches.iter().any(|pitch| pitch.midi_note == 48));
        assert!(pitches.iter().any(|pitch| pitch.midi_note == 60));
    }
}
