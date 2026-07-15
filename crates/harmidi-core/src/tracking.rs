use crate::analysis::{DetectedNote, FrameSummary};

#[derive(Debug)]
struct ActiveNote {
    start_frame: usize,
    last_seen_frame: usize,
    confidence_sum: f32,
    confidence_peak: f32,
    cents_weighted_sum: f32,
    weight_sum: f32,
    gap_frames: usize,
}

pub fn track_notes(
    frames: &[FrameSummary],
    min_midi: u8,
    max_midi: u8,
    hop_seconds: f32,
    min_note_ms: f32,
) -> Vec<DetectedNote> {
    let mut notes = Vec::new();
    let max_gap_frames = 2_usize;
    let minimum_frames = ((min_note_ms / 1000.0) / hop_seconds).ceil().max(1.0) as usize;

    for midi in min_midi..=max_midi {
        let mut active: Option<ActiveNote> = None;

        for (frame_index, frame) in frames.iter().enumerate() {
            let pitch = frame.pitches.iter().find(|pitch| pitch.midi_note == midi);

            match (active.as_mut(), pitch) {
                (None, Some(pitch)) if pitch.confidence >= 0.22 => {
                    active = Some(ActiveNote {
                        start_frame: frame_index,
                        last_seen_frame: frame_index,
                        confidence_sum: pitch.confidence,
                        confidence_peak: pitch.confidence,
                        cents_weighted_sum: pitch.cents_offset * pitch.confidence,
                        weight_sum: pitch.confidence,
                        gap_frames: 0,
                    });
                }
                (Some(state), Some(pitch)) if pitch.confidence >= 0.12 => {
                    state.last_seen_frame = frame_index;
                    state.confidence_sum += pitch.confidence;
                    state.confidence_peak = state.confidence_peak.max(pitch.confidence);
                    state.cents_weighted_sum += pitch.cents_offset * pitch.confidence;
                    state.weight_sum += pitch.confidence;
                    state.gap_frames = 0;
                }
                (Some(state), _) => {
                    state.gap_frames += 1;
                    if state.gap_frames > max_gap_frames {
                        finalize_note(
                            &mut notes,
                            midi,
                            active.take().expect("active note must exist"),
                            hop_seconds,
                            minimum_frames,
                        );
                    }
                }
                _ => {}
            }
        }

        if let Some(state) = active.take() {
            finalize_note(
                &mut notes,
                midi,
                state,
                hop_seconds,
                minimum_frames,
            );
        }
    }

    notes.sort_by(|left, right| {
        left.start_seconds
            .total_cmp(&right.start_seconds)
            .then(left.midi_note.cmp(&right.midi_note))
    });
    merge_adjacent(notes, hop_seconds * 2.5)
}

fn finalize_note(
    notes: &mut Vec<DetectedNote>,
    midi: u8,
    state: ActiveNote,
    hop_seconds: f32,
    minimum_frames: usize,
) {
    let frame_count = state.last_seen_frame.saturating_sub(state.start_frame) + 1;
    if frame_count < minimum_frames {
        return;
    }

    let average_confidence = state.confidence_sum / frame_count as f32;
    let confidence = (average_confidence * 0.65 + state.confidence_peak * 0.35).clamp(0.0, 1.0);
    let cents = if state.weight_sum > 1.0e-6 {
        state.cents_weighted_sum / state.weight_sum
    } else {
        0.0
    };

    notes.push(DetectedNote {
        midi_note: midi,
        start_seconds: state.start_frame as f32 * hop_seconds,
        end_seconds: (state.last_seen_frame + 1) as f32 * hop_seconds,
        velocity: (30.0 + confidence.sqrt() * 97.0).round().clamp(1.0, 127.0) as u8,
        confidence,
        cents_offset: cents,
    });
}

fn merge_adjacent(notes: Vec<DetectedNote>, maximum_gap: f32) -> Vec<DetectedNote> {
    let mut merged: Vec<DetectedNote> = Vec::with_capacity(notes.len());

    for note in notes {
        if let Some(previous) = merged.last_mut()
            && previous.midi_note == note.midi_note
            && note.start_seconds - previous.end_seconds <= maximum_gap
        {
            let previous_duration = previous.end_seconds - previous.start_seconds;
            let note_duration = note.end_seconds - note.start_seconds;
            let total_duration = (previous_duration + note_duration).max(1.0e-6);
            previous.cents_offset =
                (previous.cents_offset * previous_duration + note.cents_offset * note_duration)
                    / total_duration;
            previous.confidence = previous.confidence.max(note.confidence);
            previous.velocity = previous.velocity.max(note.velocity);
            previous.end_seconds = note.end_seconds;
            continue;
        }
        merged.push(note);
    }

    merged
}
