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

fn detect_frame_pitches(
    spectrum: &[f32],
    sample_rate: u32,
    options: &AnalysisOptions,
) -> Vec<FramePitch> {
    let midi_count = options.max_midi as usize - options.min_midi as usize + 1;
    let mut scores = vec![0.0_f32; midi_count];
    let nyquist = sample_rate as f32 / 2.0;
    let bin_hz = sample_rate as f32 / options.fft_size as f32;

    for midi in options.min_midi..=options.max_midi {
        let fundamental = midi_to_frequency(midi);
        let mut weighted_sum = 0.0_f32;
        let mut weight_total = 0.0_f32;

        for harmonic in 1..=8 {
            let frequency = fundamental * harmonic as f32;
            if frequency >= nyquist {
                break;
            }
            let center_bin = (frequency / bin_hz).round() as usize;
            let (_, magnitude) = local_peak(spectrum, center_bin, 1);
            let weight = if harmonic == 1 {
                1.35
            } else {
                1.0 / (harmonic as f32).sqrt()
            };
            weighted_sum += magnitude * weight;
            weight_total += weight;
        }

        let index = midi as usize - options.min_midi as usize;
        scores[index] = if weight_total > 0.0 {
            weighted_sum / weight_total
        } else {
            0.0
        };
    }

    let maximum = scores.iter().copied().fold(0.0_f32, f32::max);
    if maximum <= 1.0e-6 {
        return Vec::new();
    }

    let threshold_ratio = 0.17 + options.sensitivity * 0.38;
    let mut candidates: Vec<(u8, f32)> = scores
        .iter()
        .enumerate()
        .filter_map(|(index, score)| {
            let left = index.checked_sub(1).map_or(0.0, |value| scores[value]);
            let right = scores.get(index + 1).copied().unwrap_or(0.0);
            let is_local_peak = *score >= left && *score >= right;
            let normalized = *score / maximum;
            (is_local_peak && normalized >= threshold_ratio)
                .then_some((options.min_midi + index as u8, normalized))
        })
        .collect();

    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));

    let mut selected: Vec<(u8, f32)> = Vec::with_capacity(options.max_polyphony);
    for candidate in candidates {
        if selected.len() >= options.max_polyphony {
            break;
        }

        let is_likely_harmonic = selected.iter().any(|(lower_midi, lower_score)| {
            if *lower_midi >= candidate.0 {
                return false;
            }
            let ratio = midi_to_frequency(candidate.0) / midi_to_frequency(*lower_midi);
            let nearest_harmonic = ratio.round();
            let close_to_harmonic = (ratio - nearest_harmonic).abs() < 0.035 && nearest_harmonic >= 2.0;
            close_to_harmonic && candidate.1 < *lower_score * 0.72
        });

        if !is_likely_harmonic {
            selected.push(candidate);
        }
    }

    selected
        .into_iter()
        .map(|(midi, confidence)| {
            let expected_frequency = midi_to_frequency(midi);
            let center_bin = (expected_frequency / bin_hz).round() as usize;
            let (peak_bin, _) = local_peak(spectrum, center_bin, 2);
            let measured_frequency = peak_bin as f32 * bin_hz;
            let measured_midi = if measured_frequency > 0.0 {
                frequency_to_midi_float(measured_frequency)
            } else {
                midi as f32
            };

            FramePitch {
                midi_note: midi,
                frequency_hz: measured_frequency,
                confidence: confidence.clamp(0.0, 1.0),
                cents_offset: ((measured_midi - midi as f32) * 100.0).clamp(-100.0, 100.0),
            }
        })
        .collect()
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

    #[test]
    fn rejects_non_power_of_two_fft_size() {
        let options = AnalysisOptions {
            fft_size: 1000,
            hop_size: 256,
            min_midi: 36,
            max_midi: 96,
            max_polyphony: 6,
            sensitivity: 0.5,
            min_note_ms: 80.0,
            target_sample_rate: 22_050,
            harmonic_enhancement: true,
        };
        assert!(validate_options(&options).is_err());
    }
}
