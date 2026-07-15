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

pub fn linear_resample(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if source_rate == target_rate || samples.len() < 2 {
        return samples.to_vec();
    }

    let ratio = target_rate as f64 / source_rate as f64;
    let output_len = ((samples.len() as f64 * ratio).round() as usize).max(2);
    let mut output = Vec::with_capacity(output_len);

    for output_index in 0..output_len {
        let source_position = output_index as f64 / ratio;
        let left = source_position.floor() as usize;
        let right = (left + 1).min(samples.len() - 1);
        let fraction = (source_position - left as f64) as f32;
        let sample = samples[left] * (1.0 - fraction) + samples[right] * fraction;
        output.push(sample);
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

    let start = center.saturating_sub(radius);
    let end = (center + radius).min(magnitudes.len() - 1);
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

pub fn median(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(|left, right| left.total_cmp(right));
    values[values.len() / 2]
}
