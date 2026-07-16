use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::dsp::{
    bandlimited_resample, frequency_to_midi_float, hann_window, local_peak, median,
    midi_to_frequency, parabolic_peak_bin, percentile, remove_dc_and_normalize, rms,
};
use crate::tracking::track_notes;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AnalysisQuality {
    Fast,
    #[default]
    Balanced,
    Accurate,
}

fn default_true() -> bool {
    true
}

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
    #[serde(default)]
    pub quality: AnalysisQuality,
    #[serde(default = "default_true")]
    pub use_hpss: bool,
    #[serde(default = "default_true")]
    pub use_multiresolution: bool,
    #[serde(default = "default_true")]
    pub use_residual: bool,
    #[serde(default = "default_true")]
    pub use_temporal_tracking: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePitch {
    pub midi_note: u8,
    pub frequency_hz: f32,
    pub confidence: f32,
    pub cents_offset: f32,
    pub spectral_confidence: f32,
    pub harmonic_confidence: f32,
    pub multiresolution_confidence: f32,
    pub temporal_confidence: f32,
    pub independence_confidence: f32,
    pub snr_db: f32,
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
    pub spectral_confidence: f32,
    pub harmonic_confidence: f32,
    pub temporal_confidence: f32,
    pub independence_confidence: f32,
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
    pub quality: AnalysisQuality,
    pub resolution_fft_sizes: Vec<usize>,
    pub hpss_enabled: bool,
    pub residual_extraction_enabled: bool,
    pub temporal_tracking_enabled: bool,
    pub resampler: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub duration_seconds: f32,
    pub notes: Vec<DetectedNote>,
    pub frames: Vec<FrameSummary>,
    pub diagnostics: AnalysisDiagnostics,
}

#[derive(Clone)]
struct Spectrogram {
    fft_size: usize,
    bin_hz: f32,
    bin_count: usize,
    frame_count: usize,
    magnitudes: Vec<f32>,
}

impl Spectrogram {
    fn frame(&self, frame_index: usize) -> &[f32] {
        let start = frame_index * self.bin_count;
        &self.magnitudes[start..start + self.bin_count]
    }
}

#[derive(Debug, Clone)]
struct PitchEvidence {
    midi: u8,
    raw_score: f32,
    direct: f32,
    harmonic_coverage: f32,
    resolution_support: f32,
    snr_db: f32,
    spectral_confidence: f32,
    harmonic_confidence: f32,
    multiresolution_confidence: f32,
    independence_confidence: f32,
    adjusted: f32,
}

#[derive(Debug, Clone, Copy)]
struct ResolutionEvidence {
    score: f32,
    direct: f32,
    harmonic_coverage: f32,
    snr_db: f32,
}

pub fn analyze(
    input_samples: &[f32],
    input_sample_rate: u32,
    options: &AnalysisOptions,
) -> Result<AnalysisResult, String> {
    validate_options(options)?;

    let analyzed_rate = options.target_sample_rate.min(input_sample_rate).max(8_000);
    let mut samples = bandlimited_resample(input_samples, input_sample_rate, analyzed_rate);
    remove_dc_and_normalize(&mut samples);

    let resolution_fft_sizes = resolution_sizes(options);
    let maximum_fft = resolution_fft_sizes.iter().copied().max().unwrap_or(options.fft_size);
    if samples.len() < maximum_fft {
        samples.resize(maximum_fft, 0.0);
    }

    let frame_count = 1 + samples.len().saturating_sub(options.fft_size) / options.hop_size;
    let mut planner = FftPlanner::<f32>::new();
    let mut spectrograms = Vec::with_capacity(resolution_fft_sizes.len());
    for fft_size in &resolution_fft_sizes {
        spectrograms.push(compute_spectrogram(
            &samples,
            analyzed_rate,
            *fft_size,
            options.hop_size,
            frame_count,
            &mut planner,
        ));
    }

    let primary_index = spectrograms
        .iter()
        .position(|spectrogram| spectrogram.fft_size == options.fft_size)
        .unwrap_or(0);
    let mut primary_harmonic = spectrograms[primary_index].clone();
    let mut percussive_energy = vec![0.0_f32; frame_count];
    if options.harmonic_enhancement && options.use_hpss {
        let (harmonic, percussive) = apply_hpss(&spectrograms[primary_index], options.quality);
        primary_harmonic = harmonic;
        percussive_energy = percussive;
    }

    let frame_rms = compute_frame_rms(&samples, options.fft_size, options.hop_size, frame_count);
    let silence_floor = adaptive_silence_floor(&frame_rms);
    let mut raw_onsets = compute_onsets(&spectrograms[primary_index], &percussive_energy);
    normalize_onsets(&mut raw_onsets);

    let mut frames = Vec::with_capacity(frame_count);
    let hop_seconds = options.hop_size as f32 / analyzed_rate as f32;
    for frame_index in 0..frame_count {
        let pitches = if frame_rms[frame_index] >= silence_floor {
            detect_frame_pitches(
                frame_index,
                &primary_harmonic,
                &spectrograms,
                primary_index,
                analyzed_rate,
                options,
            )
        } else {
            Vec::new()
        };

        frames.push(FrameSummary {
            time_seconds: frame_index as f32 * hop_seconds,
            rms: frame_rms[frame_index],
            onset_strength: raw_onsets[frame_index],
            pitches,
        });
    }

    refine_temporal_confidence(&mut frames, options.max_polyphony, options.sensitivity);

    let notes = track_notes(
        &frames,
        options.min_midi,
        options.max_midi,
        options.max_polyphony,
        hop_seconds,
        options.min_note_ms,
        options.use_temporal_tracking,
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
            quality: options.quality,
            resolution_fft_sizes,
            hpss_enabled: options.harmonic_enhancement && options.use_hpss,
            residual_extraction_enabled: options.use_residual,
            temporal_tracking_enabled: options.use_temporal_tracking,
            resampler: "windowed-sinc",
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
    if !(8_000..=48_000).contains(&options.target_sample_rate) {
        return Err("targetSampleRate must be between 8000 and 48000".into());
    }
    Ok(())
}

fn resolution_sizes(options: &AnalysisOptions) -> Vec<usize> {
    if !options.use_multiresolution || matches!(options.quality, AnalysisQuality::Fast) {
        return vec![options.fft_size];
    }

    let mut sizes = match options.quality {
        AnalysisQuality::Fast => vec![options.fft_size],
        AnalysisQuality::Balanced => vec![
            (options.fft_size / 2).max(2048),
            options.fft_size,
            (options.fft_size * 2).min(16_384),
        ],
        AnalysisQuality::Accurate => vec![
            2048,
            4096,
            8192,
            16_384,
            options.fft_size,
        ],
    };
    sizes.retain(|size| (512..=16_384).contains(size) && size.is_power_of_two());
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

fn compute_spectrogram(
    samples: &[f32],
    sample_rate: u32,
    fft_size: usize,
    hop_size: usize,
    frame_count: usize,
    planner: &mut FftPlanner<f32>,
) -> Spectrogram {
    let bin_count = fft_size / 2 + 1;
    let window = hann_window(fft_size);
    let fft: Arc<dyn Fft<f32>> = planner.plan_fft_forward(fft_size);
    let mut buffer = vec![Complex32::new(0.0, 0.0); fft_size];
    let mut magnitudes = vec![0.0_f32; frame_count * bin_count];

    for frame_index in 0..frame_count {
        let start = frame_index * hop_size;
        for (sample_index, target) in buffer.iter_mut().enumerate() {
            let sample = samples.get(start + sample_index).copied().unwrap_or(0.0);
            *target = Complex32::new(sample * window[sample_index], 0.0);
        }
        fft.process(&mut buffer);

        let output = &mut magnitudes[frame_index * bin_count..(frame_index + 1) * bin_count];
        for (bin, value) in output.iter_mut().enumerate() {
            *value = buffer[bin].norm() / fft_size as f32;
        }
    }

    Spectrogram {
        fft_size,
        bin_hz: sample_rate as f32 / fft_size as f32,
        bin_count,
        frame_count,
        magnitudes,
    }
}

fn compute_frame_rms(
    samples: &[f32],
    fft_size: usize,
    hop_size: usize,
    frame_count: usize,
) -> Vec<f32> {
    (0..frame_count)
        .map(|frame_index| {
            let start = frame_index * hop_size;
            let end = (start + fft_size).min(samples.len());
            rms(&samples[start.min(samples.len())..end])
        })
        .collect()
}

fn adaptive_silence_floor(frame_rms: &[f32]) -> f32 {
    let mut values = frame_rms.to_vec();
    let noise = percentile(&mut values, 0.20);
    let mut values = frame_rms.to_vec();
    let typical = percentile(&mut values, 0.65);
    (noise * 2.2).max(typical * 0.035).max(0.0008)
}

fn apply_hpss(primary: &Spectrogram, quality: AnalysisQuality) -> (Spectrogram, Vec<f32>) {
    let (time_radius, frequency_radius) = match quality {
        AnalysisQuality::Fast => (2_usize, 4_usize),
        AnalysisQuality::Balanced => (4, 8),
        AnalysisQuality::Accurate => (7, 14),
    };
    let mut harmonic = primary.clone();
    let mut percussive_energy = vec![0.0_f32; primary.frame_count];
    let mut time_values = Vec::with_capacity(time_radius * 2 + 1);
    let mut frequency_values = Vec::with_capacity(frequency_radius * 2 + 1);

    for frame in 0..primary.frame_count {
        for bin in 0..primary.bin_count {
            time_values.clear();
            let first_frame = frame.saturating_sub(time_radius);
            let last_frame = (frame + time_radius).min(primary.frame_count - 1);
            for neighbor_frame in first_frame..=last_frame {
                time_values.push(primary.magnitudes[neighbor_frame * primary.bin_count + bin]);
            }

            frequency_values.clear();
            let first_bin = bin.saturating_sub(frequency_radius);
            let last_bin = (bin + frequency_radius).min(primary.bin_count - 1);
            let frame_offset = frame * primary.bin_count;
            for neighbor_bin in first_bin..=last_bin {
                frequency_values.push(primary.magnitudes[frame_offset + neighbor_bin]);
            }

            let horizontal = median(&mut time_values);
            let vertical = median(&mut frequency_values);
            let h_power = horizontal * horizontal;
            let p_power = vertical * vertical;
            let mask = h_power / (h_power + p_power + 1.0e-12);
            let current = primary.magnitudes[frame_offset + bin];
            harmonic.magnitudes[frame_offset + bin] = current * mask.sqrt();
            percussive_energy[frame] += current * (1.0 - mask).sqrt();
        }
        percussive_energy[frame] /= primary.bin_count.max(1) as f32;
    }

    (harmonic, percussive_energy)
}

fn compute_onsets(primary: &Spectrogram, percussive_energy: &[f32]) -> Vec<f32> {
    let mut onsets = vec![0.0_f32; primary.frame_count];
    for frame in 1..primary.frame_count {
        let current = primary.frame(frame);
        let previous = primary.frame(frame - 1);
        let mut flux = 0.0_f32;
        let mut weight_sum = 0.0_f32;
        for bin in 1..primary.bin_count {
            let frequency_weight = (bin as f32 / primary.bin_count as f32).sqrt().max(0.15);
            flux += (current[bin] - previous[bin]).max(0.0) * frequency_weight;
            weight_sum += frequency_weight;
        }
        let spectral_flux = flux / weight_sum.max(1.0e-6);
        let percussive_change = (percussive_energy[frame] - percussive_energy[frame - 1]).max(0.0);
        onsets[frame] = spectral_flux * 0.68 + percussive_change * 0.32;
    }
    onsets
}

fn normalize_onsets(onsets: &mut [f32]) {
    let mut values = onsets.to_vec();
    let floor = percentile(&mut values, 0.50);
    let mut values = onsets.to_vec();
    let high = percentile(&mut values, 0.95).max(floor + 1.0e-8);
    for onset in onsets {
        *onset = ((*onset - floor) / (high - floor)).clamp(0.0, 1.0);
    }
}

fn detect_frame_pitches(
    frame_index: usize,
    primary: &Spectrogram,
    spectrograms: &[Spectrogram],
    primary_index: usize,
    sample_rate: u32,
    options: &AnalysisOptions,
) -> Vec<FramePitch> {
    let midi_count = options.max_midi as usize - options.min_midi as usize + 1;
    let mut evidence = Vec::with_capacity(midi_count);

    for midi in options.min_midi..=options.max_midi {
        let mut resolution_values = Vec::with_capacity(spectrograms.len());
        for (index, spectrogram) in spectrograms.iter().enumerate() {
            let spectrum = if index == primary_index {
                primary.frame(frame_index)
            } else {
                spectrogram.frame(frame_index)
            };
            resolution_values.push(score_pitch_on_spectrum(
                spectrum,
                spectrogram,
                midi,
                sample_rate,
            ));
        }
        evidence.push(fuse_resolution_evidence(midi, &resolution_values));
    }

    let maximum = evidence
        .iter()
        .map(|candidate| candidate.raw_score)
        .fold(0.0_f32, f32::max);
    if maximum <= 1.0e-9 {
        return Vec::new();
    }

    for candidate in &mut evidence {
        candidate.spectral_confidence = (candidate.raw_score / maximum).clamp(0.0, 1.0);
        candidate.harmonic_confidence = candidate.harmonic_coverage.clamp(0.0, 1.0);
        candidate.multiresolution_confidence = candidate.resolution_support.clamp(0.0, 1.0);
        let snr_confidence = ((candidate.snr_db - 2.0) / 22.0).clamp(0.0, 1.0);
        candidate.adjusted = candidate.spectral_confidence * 0.37
            + candidate.harmonic_confidence * 0.25
            + candidate.multiresolution_confidence * 0.20
            + snr_confidence * 0.18;
        candidate.independence_confidence = 1.0;
    }

    apply_harmonic_competition(&mut evidence);
    let threshold = confidence_threshold(options.sensitivity, options.quality);
    let primary_spectrum = primary.frame(frame_index);
    let mut residual = primary_spectrum.to_vec();
    let mut selected = Vec::<PitchEvidence>::with_capacity(options.max_polyphony);
    let mut blocked = vec![false; evidence.len()];

    while selected.len() < options.max_polyphony {
        let mut best: Option<(usize, f32)> = None;
        for (index, candidate) in evidence.iter().enumerate() {
            if blocked[index] || candidate.adjusted < threshold * 0.72 {
                continue;
            }
            if selected
                .iter()
                .any(|existing| existing.midi.abs_diff(candidate.midi) <= 1)
            {
                continue;
            }

            let residual_evidence = score_pitch_on_spectrum(
                &residual,
                primary,
                candidate.midi,
                sample_rate,
            );
            let residual_scale = (residual_evidence.score / candidate.raw_score.max(1.0e-9))
                .clamp(0.0, 1.25);
            let residual_confidence = if options.use_residual {
                residual_scale.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let score = candidate.adjusted * (0.72 + residual_confidence * 0.28);
            if best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((index, score));
            }
        }

        let Some((best_index, best_score)) = best else {
            break;
        };
        if best_score < threshold {
            break;
        }

        let mut chosen = evidence[best_index].clone();
        chosen.adjusted = best_score;
        chosen.independence_confidence = (best_score / evidence[best_index].adjusted.max(1.0e-6))
            .clamp(0.0, 1.0);
        if options.use_residual {
            subtract_harmonic_template(&mut residual, primary, &chosen, 0.66);
        }
        blocked[best_index] = true;
        selected.push(chosen);
    }

    selected.sort_by_key(|candidate| candidate.midi);
    selected
        .into_iter()
        .map(|candidate| evidence_to_frame_pitch(candidate, primary_spectrum, primary))
        .collect()
}

fn score_pitch_on_spectrum(
    spectrum: &[f32],
    spectrogram: &Spectrogram,
    midi: u8,
    sample_rate: u32,
) -> ResolutionEvidence {
    let fundamental = midi_to_frequency(midi);
    let nyquist = sample_rate as f32 / 2.0;
    let mut weighted_sum = 0.0_f32;
    let mut weight_total = 0.0_f32;
    let mut direct = 0.0_f32;
    let mut covered = 0.0_f32;
    let mut possible = 0.0_f32;
    let mut noise_samples = spectrum.to_vec();
    let global_floor = percentile(&mut noise_samples, 0.55).max(1.0e-9);

    for harmonic in 1..=12 {
        let frequency = fundamental * harmonic as f32;
        if frequency >= nyquist {
            break;
        }
        let contrast = log_frequency_contrast(spectrum, spectrogram.bin_hz, frequency);
        if harmonic == 1 {
            direct = contrast;
        }
        let weight = if harmonic == 1 {
            1.85
        } else {
            1.0 / (harmonic as f32).powf(0.78)
        };
        weighted_sum += contrast * weight;
        weight_total += weight;
        possible += 1.0;
        if contrast > global_floor * 1.8 {
            covered += 1.0;
        }
    }

    let cycles = fundamental * spectrogram.fft_size as f32 / sample_rate as f32;
    let resolution_weight = (-((cycles.max(1.0) / 14.0).ln().powi(2)) / 2.2).exp();
    let coverage = if possible > 0.0 { covered / possible } else { 0.0 };
    let harmonic_mean = weighted_sum / weight_total.max(1.0e-6);
    let score = (direct * 1.30 + harmonic_mean + coverage * global_floor * 0.6)
        * (0.55 + 0.45 * resolution_weight);
    let snr_db = 20.0 * ((direct + global_floor) / global_floor).log10();

    ResolutionEvidence {
        score,
        direct,
        harmonic_coverage: coverage,
        snr_db,
    }
}

fn fuse_resolution_evidence(midi: u8, values: &[ResolutionEvidence]) -> PitchEvidence {
    let mut scores: Vec<f32> = values.iter().map(|value| value.score).collect();
    scores.sort_unstable_by(|left, right| right.total_cmp(left));
    let best = scores.first().copied().unwrap_or(0.0);
    let second = scores.get(1).copied().unwrap_or(best);
    let mean = scores.iter().copied().sum::<f32>() / scores.len().max(1) as f32;
    let support = if best > 1.0e-9 {
        scores.iter().filter(|score| **score >= best * 0.38).count() as f32
            / scores.len().max(1) as f32
    } else {
        0.0
    };

    PitchEvidence {
        midi,
        raw_score: best * 0.48 + second * 0.30 + mean * 0.22,
        direct: values.iter().map(|value| value.direct).fold(0.0_f32, f32::max),
        harmonic_coverage: values
            .iter()
            .map(|value| value.harmonic_coverage)
            .sum::<f32>()
            / values.len().max(1) as f32,
        resolution_support: support,
        snr_db: values.iter().map(|value| value.snr_db).fold(0.0_f32, f32::max),
        spectral_confidence: 0.0,
        harmonic_confidence: 0.0,
        multiresolution_confidence: 0.0,
        independence_confidence: 1.0,
        adjusted: 0.0,
    }
}

fn log_frequency_contrast(spectrum: &[f32], bin_hz: f32, frequency: f32) -> f32 {
    if spectrum.is_empty() || frequency <= 0.0 {
        return 0.0;
    }

    let center = frequency / bin_hz;
    let inner_low = frequency * 2.0_f32.powf(-35.0 / 1200.0) / bin_hz;
    let inner_high = frequency * 2.0_f32.powf(35.0 / 1200.0) / bin_hz;
    let start = inner_low.floor().max(0.0) as usize;
    let end = inner_high.ceil().min((spectrum.len() - 1) as f32) as usize;
    let center_bin = center.round().clamp(0.0, (spectrum.len() - 1) as f32) as usize;
    let (_, local_max) = local_peak(spectrum, center_bin, ((end.saturating_sub(start)) / 2).max(1));

    let outer_low = frequency * 2.0_f32.powf(-190.0 / 1200.0) / bin_hz;
    let outer_high = frequency * 2.0_f32.powf(190.0 / 1200.0) / bin_hz;
    let outer_start = outer_low.floor().max(0.0) as usize;
    let outer_end = outer_high.ceil().min((spectrum.len() - 1) as f32) as usize;
    let mut floor_values = Vec::with_capacity(outer_end.saturating_sub(outer_start));
    for (bin, value) in spectrum
        .iter()
        .enumerate()
        .take(outer_end + 1)
        .skip(outer_start)
    {
        if bin < start.saturating_sub(1) || bin > end + 1 {
            floor_values.push(*value);
        }
    }
    let floor = median(&mut floor_values);
    (local_max - floor).max(0.0)
}

fn apply_harmonic_competition(candidates: &mut [PitchEvidence]) {
    let snapshot = candidates.to_vec();
    for candidate in candidates {
        let mut penalty = 1.0_f32;
        for lower in &snapshot {
            if lower.midi >= candidate.midi || lower.spectral_confidence < 0.16 {
                continue;
            }
            let Some(order) = harmonic_order(lower.midi, candidate.midi) else {
                continue;
            };
            let lower_strength = lower.adjusted / candidate.adjusted.max(1.0e-6);
            let independent = candidate.direct / lower.direct.max(1.0e-8);
            let support = candidate.multiresolution_confidence;
            let independent_threshold = if order == 2 { 0.95 } else { 0.72 };
            if independent >= independent_threshold && support >= 0.66 {
                continue;
            }

            let dominance = ((lower_strength - 0.35) / 1.15).clamp(0.0, 1.0);
            let maximum_suppression = if order <= 4 { 0.88 } else { 0.94 };
            penalty *= (1.0 - maximum_suppression * dominance).clamp(0.03, 1.0);
        }
        candidate.independence_confidence = penalty;
        candidate.adjusted *= penalty;
    }
}

fn harmonic_order(lower_midi: u8, upper_midi: u8) -> Option<usize> {
    let interval = upper_midi as f32 - lower_midi as f32;
    let mut best: Option<(usize, f32)> = None;
    for order in 2..=12 {
        let expected_interval = 12.0 * (order as f32).log2();
        let error = (interval - expected_interval).abs();
        if error <= 0.52 && best.is_none_or(|(_, best_error)| error < best_error) {
            best = Some((order, error));
        }
    }
    best.map(|(order, _)| order)
}

fn subtract_harmonic_template(
    residual: &mut [f32],
    spectrogram: &Spectrogram,
    candidate: &PitchEvidence,
    amount: f32,
) {
    let fundamental = midi_to_frequency(candidate.midi);
    for harmonic in 1..=14 {
        let frequency = fundamental * harmonic as f32;
        let center = (frequency / spectrogram.bin_hz).round() as usize;
        if center >= residual.len() {
            break;
        }
        let (_, peak) = local_peak(residual, center, 1);
        let decay = 1.0 / (harmonic as f32).powf(0.72);
        let removal = peak.min(candidate.direct * decay) * amount;
        for offset in -2_i32..=2 {
            let bin = center as i32 + offset;
            if bin < 0 || bin >= residual.len() as i32 {
                continue;
            }
            let shape = match offset.abs() {
                0 => 1.0,
                1 => 0.58,
                _ => 0.22,
            };
            residual[bin as usize] = (residual[bin as usize] - removal * shape).max(0.0);
        }
    }
}

fn evidence_to_frame_pitch(
    candidate: PitchEvidence,
    spectrum: &[f32],
    spectrogram: &Spectrogram,
) -> FramePitch {
    let expected_frequency = midi_to_frequency(candidate.midi);
    let center_bin = (expected_frequency / spectrogram.bin_hz).round() as usize;
    let (peak_bin, _) = local_peak(spectrum, center_bin, 2);
    let interpolated_bin = parabolic_peak_bin(spectrum, peak_bin);
    let measured_frequency = interpolated_bin * spectrogram.bin_hz;
    let measured_midi = if measured_frequency > 0.0 {
        frequency_to_midi_float(measured_frequency)
    } else {
        candidate.midi as f32
    };

    FramePitch {
        midi_note: candidate.midi,
        frequency_hz: measured_frequency,
        confidence: candidate.adjusted.clamp(0.0, 1.0),
        cents_offset: ((measured_midi - candidate.midi as f32) * 100.0).clamp(-100.0, 100.0),
        spectral_confidence: candidate.spectral_confidence,
        harmonic_confidence: candidate.harmonic_confidence,
        multiresolution_confidence: candidate.multiresolution_confidence,
        temporal_confidence: 0.0,
        independence_confidence: candidate.independence_confidence,
        snr_db: candidate.snr_db,
    }
}

fn confidence_threshold(sensitivity: f32, quality: AnalysisQuality) -> f32 {
    let base = 0.62 - sensitivity * 0.36;
    match quality {
        AnalysisQuality::Fast => base + 0.04,
        AnalysisQuality::Balanced => base,
        AnalysisQuality::Accurate => base - 0.025,
    }
    .clamp(0.20, 0.68)
}

fn refine_temporal_confidence(frames: &mut [FrameSummary], max_polyphony: usize, sensitivity: f32) {
    let snapshot = frames.to_vec();
    for (frame_index, frame) in frames.iter_mut().enumerate() {
        for pitch in &mut frame.pitches {
            let first = frame_index.saturating_sub(2);
            let last = (frame_index + 2).min(snapshot.len() - 1);
            let mut support = 0.0_f32;
            let mut possible = 0.0_f32;
            for neighbor_frame in &snapshot[first..=last] {
                possible += 1.0;
                let neighbor = neighbor_frame
                    .pitches
                    .iter()
                    .filter(|neighbor| neighbor.midi_note.abs_diff(pitch.midi_note) <= 1)
                    .map(|neighbor| {
                        if neighbor.midi_note == pitch.midi_note {
                            neighbor.confidence
                        } else {
                            neighbor.confidence * 0.42
                        }
                    })
                    .fold(0.0_f32, f32::max);
                support += neighbor;
            }
            pitch.temporal_confidence = (support / possible.max(1.0)).clamp(0.0, 1.0);
            let onset_bonus = frame.onset_strength * 0.10;
            let isolated_penalty = if pitch.temporal_confidence < 0.18 && frame.onset_strength < 0.25 {
                0.62
            } else {
                1.0
            };
            pitch.confidence = (pitch.confidence * 0.74
                + pitch.temporal_confidence * 0.24
                + onset_bonus)
                .clamp(0.0, 1.0)
                * isolated_penalty;
        }

        let keep_threshold = (0.24 - sensitivity * 0.10).clamp(0.10, 0.24);
        frame.pitches.retain(|pitch| pitch.confidence >= keep_threshold);
        frame.pitches.sort_by(|left, right| right.confidence.total_cmp(&left.confidence));
        frame.pitches.truncate(max_polyphony);
        frame.pitches.sort_by_key(|pitch| pitch.midi_note);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn default_options() -> AnalysisOptions {
        AnalysisOptions {
            fft_size: 4096,
            hop_size: 512,
            min_midi: 36,
            max_midi: 96,
            max_polyphony: 8,
            sensitivity: 0.58,
            min_note_ms: 90.0,
            target_sample_rate: 22_050,
            harmonic_enhancement: true,
            quality: AnalysisQuality::Fast,
            use_hpss: true,
            use_multiresolution: false,
            use_residual: true,
            use_temporal_tracking: true,
        }
    }

    fn harmonic_tone(midi: u8, duration: f32, sample_rate: u32, amplitude: f32) -> Vec<f32> {
        let length = (duration * sample_rate as f32) as usize;
        let fundamental = midi_to_frequency(midi);
        (0..length)
            .map(|index| {
                let time = index as f32 / sample_rate as f32;
                let attack = (time / 0.025).min(1.0);
                let release = ((duration - time) / 0.06).clamp(0.0, 1.0);
                let mut sample = 0.0;
                for harmonic in 1..=8 {
                    sample += (2.0 * PI * fundamental * harmonic as f32 * time).sin()
                        * amplitude
                        / (harmonic as f32).powf(0.82);
                }
                sample * attack * release * 0.35
            })
            .collect()
    }

    fn mix(buffers: &[Vec<f32>]) -> Vec<f32> {
        let length = buffers.iter().map(Vec::len).max().unwrap_or(0);
        let mut output = vec![0.0_f32; length];
        for buffer in buffers {
            for (target, source) in output.iter_mut().zip(buffer) {
                *target += *source;
            }
        }
        output
    }

    #[test]
    fn rejects_non_power_of_two_fft_size() {
        let mut options = default_options();
        options.fft_size = 1000;
        assert!(validate_options(&options).is_err());
    }

    #[test]
    fn detects_fundamental_without_registering_its_harmonics() {
        let options = default_options();
        let audio = harmonic_tone(48, 0.8, options.target_sample_rate, 1.0);
        let result = analyze(&audio, options.target_sample_rate, &options).unwrap();
        eprintln!("single notes: {:?}", result.notes);
        eprintln!("single frames: {:?}", result.frames.iter().filter(|frame| !frame.pitches.is_empty()).take(8).map(|frame| &frame.pitches).collect::<Vec<_>>());
        assert!(result.notes.iter().any(|note| note.midi_note == 48));
        assert!(!result.notes.iter().any(|note| note.midi_note == 60));
        assert!(!result.notes.iter().any(|note| note.midi_note == 67));
    }

    #[test]
    fn preserves_c_major_chord_fundamentals() {
        let options = default_options();
        let audio = mix(&[
            harmonic_tone(60, 0.9, options.target_sample_rate, 1.0),
            harmonic_tone(64, 0.9, options.target_sample_rate, 0.88),
            harmonic_tone(67, 0.9, options.target_sample_rate, 0.82),
        ]);
        let result = analyze(&audio, options.target_sample_rate, &options).unwrap();
        eprintln!("chord notes: {:?}", result.notes);
        eprintln!("chord frames: {:?}", result.frames.iter().filter(|frame| !frame.pitches.is_empty()).take(8).map(|frame| &frame.pitches).collect::<Vec<_>>());
        for midi in [60, 64, 67] {
            assert!(result.notes.iter().any(|note| note.midi_note == midi), "missing {midi}");
        }
        assert!(!result.notes.iter().any(|note| note.midi_note == 72));
    }

    #[test]
    fn rejects_short_impulsive_noise_as_sustained_notes() {
        let options = default_options();
        let mut audio = vec![0.0_f32; options.target_sample_rate as usize];
        for index in (1000..audio.len()).step_by(3200) {
            audio[index] = 1.0;
            if index + 1 < audio.len() {
                audio[index + 1] = -0.8;
            }
        }
        let result = analyze(&audio, options.target_sample_rate, &options).unwrap();
        assert!(result.notes.len() <= 2);
    }

    #[test]
    fn confidence_components_are_bounded() {
        let options = default_options();
        let audio = harmonic_tone(57, 0.7, options.target_sample_rate, 1.0);
        let result = analyze(&audio, options.target_sample_rate, &options).unwrap();
        for frame in result.frames {
            for pitch in frame.pitches {
                assert!((0.0..=1.0).contains(&pitch.confidence));
                assert!((0.0..=1.0).contains(&pitch.spectral_confidence));
                assert!((0.0..=1.0).contains(&pitch.harmonic_confidence));
                assert!((0.0..=1.0).contains(&pitch.temporal_confidence));
                assert!((0.0..=1.0).contains(&pitch.independence_confidence));
            }
        }
    }
}
