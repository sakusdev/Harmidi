use crate::analysis::{DetectedNote, FramePitch, FrameSummary};

#[derive(Debug, Clone, Copy, Default)]
struct ComponentSums {
    confidence: f32,
    spectral: f32,
    harmonic: f32,
    temporal: f32,
    independence: f32,
    cents_weighted: f32,
    weight: f32,
    samples: usize,
    peak: f32,
}

pub fn track_notes(
    frames: &[FrameSummary],
    min_midi: u8,
    max_midi: u8,
    max_polyphony: usize,
    hop_seconds: f32,
    min_note_ms: f32,
    use_viterbi: bool,
) -> Vec<DetectedNote> {
    if frames.is_empty() {
        return Vec::new();
    }

    let minimum_frames = ((min_note_ms / 1000.0) / hop_seconds).ceil().max(1.0) as usize;
    let mut notes = Vec::new();

    for midi in min_midi..=max_midi {
        let observations: Vec<f32> = frames
            .iter()
            .map(|frame| observation_for_midi(frame, midi))
            .collect();
        let path = if use_viterbi {
            viterbi_path(frames, &observations)
        } else {
            hysteresis_path(&observations)
        };
        collect_note_segments(
            &mut notes,
            frames,
            &observations,
            &path,
            midi,
            hop_seconds,
            minimum_frames,
        );
    }

    notes = merge_adjacent(notes, hop_seconds * 2.5);
    notes = suppress_harmonic_note_duplicates(notes, hop_seconds);
    notes = enforce_polyphony(notes, max_polyphony, hop_seconds);
    notes.sort_by(|left, right| {
        left.start_seconds
            .total_cmp(&right.start_seconds)
            .then(left.midi_note.cmp(&right.midi_note))
    });
    notes
}

fn observation_for_midi(frame: &FrameSummary, midi: u8) -> f32 {
    frame
        .pitches
        .iter()
        .filter(|pitch| pitch.midi_note.abs_diff(midi) <= 1)
        .map(|pitch| {
            if pitch.midi_note == midi {
                pitch.confidence
            } else {
                pitch.confidence * 0.34
            }
        })
        .fold(0.0_f32, f32::max)
        .clamp(0.0, 1.0)
}

fn safe_ln(probability: f32) -> f32 {
    probability.clamp(0.001, 0.999).ln()
}

// Frame confidence is a calibrated detector score, not a Bernoulli
// probability. Map the detector's useful 0.12-0.45 range into a
// state probability before applying Viterbi emissions.
fn observation_probability(observation: f32) -> f32 {
    let logit = ((observation - 0.18) / 0.16).clamp(-6.0, 6.0);
    (1.0 / (1.0 + (-logit).exp())).clamp(0.01, 0.99)
}

fn viterbi_path(frames: &[FrameSummary], observations: &[f32]) -> Vec<bool> {
    let count = observations.len();
    if count == 0 {
        return Vec::new();
    }

    let mut back_off = vec![false; count];
    let mut back_on = vec![false; count];
    let first = observation_probability(observations[0]);
    let mut off_score = safe_ln(1.0 - first);
    let mut on_score = safe_ln(first) - 1.10 + frames[0].onset_strength * 1.05;

    for frame_index in 1..count {
        let probability = observation_probability(observations[frame_index]);
        let onset = frames[frame_index].onset_strength;
        let emission_on = safe_ln(0.025 + probability * 0.95);
        let emission_off = safe_ln(0.025 + (1.0 - probability) * 0.95);

        let off_from_off = off_score;
        let off_from_on = on_score - (0.72 + probability * 0.55);
        let (next_off, previous_off_was_on) = if off_from_on > off_from_off {
            (off_from_on + emission_off, true)
        } else {
            (off_from_off + emission_off, false)
        };

        let start_penalty = 1.12 - onset * 0.98;
        let on_from_off = off_score - start_penalty;
        let continuity_bonus = if probability >= 0.50 { 0.18 } else { -0.12 };
        let on_from_on = on_score + continuity_bonus;
        let (next_on, previous_on_was_on) = if on_from_on >= on_from_off {
            (on_from_on + emission_on, true)
        } else {
            (on_from_off + emission_on, false)
        };

        back_off[frame_index] = previous_off_was_on;
        back_on[frame_index] = previous_on_was_on;
        off_score = next_off;
        on_score = next_on;
    }

    let mut path = vec![false; count];
    let mut state_on = on_score > off_score;
    for frame_index in (0..count).rev() {
        path[frame_index] = state_on;
        if frame_index == 0 {
            break;
        }
        state_on = if state_on {
            back_on[frame_index]
        } else {
            back_off[frame_index]
        };
    }
    path
}

fn hysteresis_path(observations: &[f32]) -> Vec<bool> {
    let mut active = false;
    observations
        .iter()
        .map(|probability| {
            if active {
                active = *probability >= 0.12;
            } else {
                active = *probability >= 0.24;
            }
            active
        })
        .collect()
}

fn collect_note_segments(
    notes: &mut Vec<DetectedNote>,
    frames: &[FrameSummary],
    observations: &[f32],
    path: &[bool],
    midi: u8,
    hop_seconds: f32,
    minimum_frames: usize,
) {
    let mut start: Option<usize> = None;
    let mut sums = ComponentSums::default();

    for frame_index in 0..path.len() {
        if path[frame_index] {
            let should_rearticulate = start.is_some()
                && frame_index.saturating_sub(start.unwrap_or(frame_index)) >= minimum_frames
                && frames[frame_index].onset_strength >= 0.78
                && observations[frame_index.saturating_sub(1)]
                    < observations[frame_index] * 0.62;
            if should_rearticulate {
                finalize_note(
                    notes,
                    midi,
                    start.take().unwrap_or(frame_index),
                    frame_index.saturating_sub(1),
                    sums,
                    hop_seconds,
                    minimum_frames,
                );
                sums = ComponentSums::default();
            }

            if start.is_none() {
                start = Some(frame_index);
            }
            accumulate_frame(&mut sums, &frames[frame_index], midi, observations[frame_index]);
        } else if let Some(start_frame) = start.take() {
            finalize_note(
                notes,
                midi,
                start_frame,
                frame_index.saturating_sub(1),
                sums,
                hop_seconds,
                minimum_frames,
            );
            sums = ComponentSums::default();
        }
    }

    if let Some(start_frame) = start {
        finalize_note(
            notes,
            midi,
            start_frame,
            path.len() - 1,
            sums,
            hop_seconds,
            minimum_frames,
        );
    }
}

fn best_pitch_for_midi(frame: &FrameSummary, midi: u8) -> Option<&FramePitch> {
    frame
        .pitches
        .iter()
        .filter(|pitch| pitch.midi_note.abs_diff(midi) <= 1)
        .max_by(|left, right| {
            let left_score = if left.midi_note == midi {
                left.confidence
            } else {
                left.confidence * 0.34
            };
            let right_score = if right.midi_note == midi {
                right.confidence
            } else {
                right.confidence * 0.34
            };
            left_score.total_cmp(&right_score)
        })
}

fn accumulate_frame(sums: &mut ComponentSums, frame: &FrameSummary, midi: u8, observation: f32) {
    sums.confidence += observation;
    sums.peak = sums.peak.max(observation);
    sums.samples += 1;

    if let Some(pitch) = best_pitch_for_midi(frame, midi) {
        let weight = pitch.confidence.max(0.05);
        sums.spectral += pitch.spectral_confidence;
        sums.harmonic += pitch.harmonic_confidence;
        sums.temporal += pitch.temporal_confidence;
        sums.independence += pitch.independence_confidence;
        sums.cents_weighted += pitch.cents_offset * weight;
        sums.weight += weight;
    }
}

fn finalize_note(
    notes: &mut Vec<DetectedNote>,
    midi: u8,
    start_frame: usize,
    end_frame: usize,
    sums: ComponentSums,
    hop_seconds: f32,
    minimum_frames: usize,
) {
    let frame_count = end_frame.saturating_sub(start_frame) + 1;
    if frame_count < minimum_frames || sums.samples == 0 {
        return;
    }

    let sample_count = sums.samples as f32;
    let average = sums.confidence / sample_count;
    let confidence = (average * 0.72 + sums.peak * 0.28).clamp(0.0, 1.0);
    if confidence < 0.13 {
        return;
    }

    let cents = if sums.weight > 1.0e-6 {
        sums.cents_weighted / sums.weight
    } else {
        0.0
    };
    let component_denominator = sample_count.max(1.0);

    notes.push(DetectedNote {
        midi_note: midi,
        start_seconds: start_frame as f32 * hop_seconds,
        end_seconds: (end_frame + 1) as f32 * hop_seconds,
        velocity: (24.0 + confidence.sqrt() * 103.0)
            .round()
            .clamp(1.0, 127.0) as u8,
        confidence,
        cents_offset: cents.clamp(-100.0, 100.0),
        spectral_confidence: (sums.spectral / component_denominator).clamp(0.0, 1.0),
        harmonic_confidence: (sums.harmonic / component_denominator).clamp(0.0, 1.0),
        temporal_confidence: (sums.temporal / component_denominator).clamp(0.0, 1.0),
        independence_confidence: (sums.independence / component_denominator).clamp(0.0, 1.0),
    });
}

fn merge_adjacent(mut notes: Vec<DetectedNote>, maximum_gap: f32) -> Vec<DetectedNote> {
    notes.sort_by(|left, right| {
        left.midi_note
            .cmp(&right.midi_note)
            .then(left.start_seconds.total_cmp(&right.start_seconds))
    });
    let mut merged: Vec<DetectedNote> = Vec::with_capacity(notes.len());

    for note in notes {
        if let Some(previous) = merged.last_mut()
            && previous.midi_note == note.midi_note
            && note.start_seconds - previous.end_seconds <= maximum_gap
        {
            let previous_duration = (previous.end_seconds - previous.start_seconds).max(1.0e-6);
            let note_duration = (note.end_seconds - note.start_seconds).max(1.0e-6);
            let total_duration = previous_duration + note_duration;
            previous.cents_offset =
                (previous.cents_offset * previous_duration + note.cents_offset * note_duration)
                    / total_duration;
            previous.confidence = (previous.confidence * previous_duration
                + note.confidence * note_duration)
                / total_duration;
            previous.spectral_confidence = previous
                .spectral_confidence
                .max(note.spectral_confidence);
            previous.harmonic_confidence = previous
                .harmonic_confidence
                .max(note.harmonic_confidence);
            previous.temporal_confidence = previous
                .temporal_confidence
                .max(note.temporal_confidence);
            previous.independence_confidence = previous
                .independence_confidence
                .max(note.independence_confidence);
            previous.velocity = previous.velocity.max(note.velocity);
            previous.end_seconds = note.end_seconds;
            continue;
        }
        merged.push(note);
    }
    merged
}

fn suppress_harmonic_note_duplicates(notes: Vec<DetectedNote>, hop_seconds: f32) -> Vec<DetectedNote> {
    let mut keep = vec![true; notes.len()];
    for upper_index in 0..notes.len() {
        for lower_index in 0..notes.len() {
            if lower_index == upper_index || notes[lower_index].midi_note >= notes[upper_index].midi_note {
                continue;
            }
            if harmonic_order(notes[lower_index].midi_note, notes[upper_index].midi_note).is_none() {
                continue;
            }

            let overlap_start = notes[lower_index].start_seconds.max(notes[upper_index].start_seconds);
            let overlap_end = notes[lower_index].end_seconds.min(notes[upper_index].end_seconds);
            let overlap = (overlap_end - overlap_start).max(0.0);
            let upper_duration = (notes[upper_index].end_seconds - notes[upper_index].start_seconds)
                .max(hop_seconds);
            let aligned = (notes[lower_index].start_seconds - notes[upper_index].start_seconds).abs()
                <= hop_seconds * 2.5;
            let likely_duplicate = overlap / upper_duration >= 0.72
                && aligned
                && notes[upper_index].confidence < notes[lower_index].confidence * 0.78
                && notes[upper_index].independence_confidence < 0.48;
            if likely_duplicate {
                keep[upper_index] = false;
                break;
            }
        }
    }

    notes
        .into_iter()
        .enumerate()
        .filter_map(|(index, note)| keep[index].then_some(note))
        .collect()
}

fn harmonic_order(lower_midi: u8, upper_midi: u8) -> Option<usize> {
    let interval = upper_midi as f32 - lower_midi as f32;
    (2..=12)
        .map(|order| {
            let expected = 12.0 * (order as f32).log2();
            (order, (interval - expected).abs())
        })
        .filter(|(_, error)| *error <= 0.52)
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(order, _)| order)
}

fn enforce_polyphony(
    notes: Vec<DetectedNote>,
    max_polyphony: usize,
    hop_seconds: f32,
) -> Vec<DetectedNote> {
    if max_polyphony == 0 || notes.len() <= max_polyphony {
        return notes;
    }

    let mut keep = vec![true; notes.len()];
    let mut event_times: Vec<f32> = notes
        .iter()
        .flat_map(|note| [note.start_seconds, note.end_seconds])
        .collect();
    event_times.sort_by(|left, right| left.total_cmp(right));
    event_times.dedup_by(|left, right| (*left - *right).abs() < hop_seconds * 0.25);

    for time in event_times {
        let mut active: Vec<usize> = notes
            .iter()
            .enumerate()
            .filter(|(index, note)| keep[*index] && note.start_seconds <= time && note.end_seconds > time)
            .map(|(index, _)| index)
            .collect();
        if active.len() <= max_polyphony {
            continue;
        }
        active.sort_by(|left, right| {
            notes[*right]
                .confidence
                .total_cmp(&notes[*left].confidence)
                .then(notes[*right].temporal_confidence.total_cmp(&notes[*left].temporal_confidence))
        });
        for index in active.into_iter().skip(max_polyphony) {
            keep[index] = false;
        }
    }

    notes
        .into_iter()
        .enumerate()
        .filter_map(|(index, note)| keep[index].then_some(note))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pitch(midi_note: u8, confidence: f32) -> FramePitch {
        FramePitch {
            midi_note,
            frequency_hz: 440.0,
            confidence,
            cents_offset: 0.0,
            spectral_confidence: confidence,
            harmonic_confidence: confidence,
            multiresolution_confidence: confidence,
            temporal_confidence: confidence,
            independence_confidence: confidence,
            snr_db: 20.0,
        }
    }

    #[test]
    fn viterbi_removes_single_frame_glitch() {
        let mut frames = Vec::new();
        for index in 0..20 {
            frames.push(FrameSummary {
                time_seconds: index as f32 * 0.02,
                rms: 0.2,
                onset_strength: if index == 2 { 1.0 } else { 0.0 },
                pitches: if index == 9 {
                    Vec::new()
                } else if (2..17).contains(&index) {
                    vec![pitch(60, 0.72)]
                } else {
                    Vec::new()
                },
            });
        }
        let notes = track_notes(&frames, 48, 72, 4, 0.02, 80.0, true);
        assert_eq!(notes.iter().filter(|note| note.midi_note == 60).count(), 1);
    }
}
