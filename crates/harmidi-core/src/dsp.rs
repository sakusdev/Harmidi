use std::f32::consts::PI;

pub fn midi_to_frequency(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((midi as f32 - 69.0) / 12.0)
}

pub fn frequency_to_midi_float(frequency: f32) -> f32 {
    69.0 + 12.0 * (frequency / 440.0).log2()
}

pub fn hann_window(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size];
    }

    (0..size)
        .map(|index| 0.5 - 0.5 * (2.0 * PI * index as f32 / (size - 1) as f32).cos())
        .collect()
}

pub fn remove_dc_and_normalize(samples: &mut [f32]) {
    if samples.is_empty() {
        return;
    }

    let mean = samples.iter().copied().sum::<f32>() / samples.len() as f32;
    let mut peak = 0.0_f32;
    for sample in samples.iter_mut() {
        *sample -= mean;
        peak = peak.max(sample.abs());
    }

    if peak > 1.0e-6 {
        let gain = 0.98 / peak;
        for sample in samples {
            *sample *= gain;
        }
    }
}

/// Band-limited windowed-sinc resampling. The previous linear interpolation
/// aliased high-frequency content into the pitch range when downsampling.
pub fn bandlimited_resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.len() < 2 {
        return samples.to_vec();
    }

    let ratio = target_rate as f64 / source_rate as f64;
    let output_len = ((samples.len() as f64 * ratio).round() as usize).max(2);
    let cutoff = (target_rate as f32 / source_rate as f32).min(1.0) * 0.94;
    let radius = if target_rate < source_rate { 20_i32 } else { 12_i32 };
    let mut output = Vec::with_capacity(output_len);

    for output_index in 0..output_len {
        let source_position = output_index as f64 / ratio;
        let center = source_position.floor() as i64;
        let mut weighted_sum = 0.0_f64;
        let mut weight_sum = 0.0_f64;

        for tap in -radius..=radius {
            let source_index = center + tap as i64;
            if source_index < 0 || source_index >= samples.len() as i64 {
                continue;
            }

            let distance = source_position - source_index as f64;
            let normalized = distance / radius as f64;
            if normalized.abs() > 1.0 {
                continue;
            }

            let x = PI as f64 * distance * cutoff as f64;
            let sinc = if x.abs() < 1.0e-10 {
                cutoff as f64
            } else {
                cutoff as f64 * x.sin() / x
            };
            let window = 0.42
                + 0.5 * (PI as f64 * normalized).cos()
                + 0.08 * (2.0 * PI as f64 * normalized).cos();
            let weight = sinc * window;
            weighted_sum += samples[source_index as usize] as f64 * weight;
            weight_sum += weight;
        }

        output.push(if weight_sum.abs() > 1.0e-12 {
            (weighted_sum / weight_sum) as f32
        } else {
            0.0
        });
    }

    output
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn local_peak(magnitudes: &[f32], center: usize, radius: usize) -> (usize, f32) {
    if magnitudes.is_empty() {
        return (0, 0.0);
    }

    let center = center.min(magnitudes.len() - 1);
    let start = center.saturating_sub(radius);
    let end = center.saturating_add(radius).min(magnitudes.len() - 1);
    let mut best_index = start;
    let mut best_value = magnitudes[start];

    for (index, value) in magnitudes.iter().enumerate().take(end + 1).skip(start) {
        if *value > best_value {
            best_index = index;
            best_value = *value;
        }
    }

    (best_index, best_value)
}

/// Returns a sub-bin peak location using quadratic interpolation.
pub fn parabolic_peak_bin(magnitudes: &[f32], peak_bin: usize) -> f32 {
    if peak_bin == 0 || peak_bin + 1 >= magnitudes.len() {
        return peak_bin as f32;
    }

    let left = magnitudes[peak_bin - 1];
    let center = magnitudes[peak_bin];
    let right = magnitudes[peak_bin + 1];
    let denominator = left - 2.0 * center + right;
    if denominator.abs() < 1.0e-12 {
        return peak_bin as f32;
    }

    let offset = (0.5 * (left - right) / denominator).clamp(-1.0, 1.0);
    peak_bin as f32 + offset
}

pub fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    values.select_nth_unstable_by(middle, |left, right| left.total_cmp(right));
    values[middle]
}

pub fn percentile(values: &mut [f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let index = ((values.len() - 1) as f32 * quantile.clamp(0.0, 1.0)).round() as usize;
    values.select_nth_unstable_by(index, |left, right| left.total_cmp(right));
    values[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parabolic_interpolation_moves_toward_stronger_neighbor() {
        let bins = [0.0, 0.7, 1.0, 0.9, 0.0];
        let peak = parabolic_peak_bin(&bins, 2);
        assert!(peak > 2.0 && peak < 2.5);
    }

    #[test]
    fn downsampling_does_not_create_large_alias() {
        let source_rate = 48_000;
        let target_rate = 12_000;
        let samples: Vec<f32> = (0..4_800)
            .map(|index| (2.0 * PI * 10_000.0 * index as f32 / source_rate as f32).sin())
            .collect();
        let output = bandlimited_resample(&samples, source_rate, target_rate);
        assert!(rms(&output) < 0.12);
    }
}
