#!/usr/bin/env node

import { readFile } from 'node:fs/promises';

const [, , referencePath, predictionPath] = process.argv;
if (!referencePath || !predictionPath) {
  console.error('Usage: npm run benchmark:notes -- reference.json prediction.json');
  process.exit(2);
}

const ONSET_TOLERANCE_SECONDS = 0.08;
const MIN_OVERLAP_RATIO = 0.20;

function validateNote(note, index, source) {
  const midiNote = Number(note.midiNote);
  const startSeconds = Number(note.startSeconds);
  const endSeconds = Number(note.endSeconds);
  if (!Number.isInteger(midiNote) || midiNote < 0 || midiNote > 127) {
    throw new Error(`${source}[${index}].midiNote must be an integer from 0 to 127`);
  }
  if (!Number.isFinite(startSeconds) || !Number.isFinite(endSeconds) || endSeconds <= startSeconds) {
    throw new Error(`${source}[${index}] must have finite startSeconds < endSeconds`);
  }
  return { ...note, midiNote, startSeconds, endSeconds };
}

async function loadNotes(path, source) {
  const parsed = JSON.parse(await readFile(path, 'utf8'));
  const notes = Array.isArray(parsed) ? parsed : parsed.notes;
  if (!Array.isArray(notes)) throw new Error(`${source} must be an array or an object containing notes[]`);
  return notes.map((note, index) => validateNote(note, index, source));
}

function overlapSeconds(left, right) {
  return Math.max(0, Math.min(left.endSeconds, right.endSeconds) - Math.max(left.startSeconds, right.startSeconds));
}

function overlapRatio(reference, prediction) {
  const overlap = overlapSeconds(reference, prediction);
  const union = Math.max(reference.endSeconds, prediction.endSeconds)
    - Math.min(reference.startSeconds, prediction.startSeconds);
  return union > 0 ? overlap / union : 0;
}

function matchNotes(references, predictions) {
  const candidates = [];
  for (let referenceIndex = 0; referenceIndex < references.length; referenceIndex += 1) {
    for (let predictionIndex = 0; predictionIndex < predictions.length; predictionIndex += 1) {
      const reference = references[referenceIndex];
      const prediction = predictions[predictionIndex];
      if (reference.midiNote !== prediction.midiNote) continue;
      const onsetError = Math.abs(reference.startSeconds - prediction.startSeconds);
      const overlap = overlapRatio(reference, prediction);
      if (onsetError > ONSET_TOLERANCE_SECONDS && overlap < MIN_OVERLAP_RATIO) continue;
      candidates.push({
        referenceIndex,
        predictionIndex,
        onsetError,
        overlap,
        cost: onsetError / ONSET_TOLERANCE_SECONDS + (1 - overlap),
      });
    }
  }

  candidates.sort((left, right) => left.cost - right.cost);
  const usedReferences = new Set();
  const usedPredictions = new Set();
  const matches = [];
  for (const candidate of candidates) {
    if (usedReferences.has(candidate.referenceIndex) || usedPredictions.has(candidate.predictionIndex)) continue;
    usedReferences.add(candidate.referenceIndex);
    usedPredictions.add(candidate.predictionIndex);
    matches.push(candidate);
  }
  return matches;
}

function mean(values) {
  return values.length === 0 ? 0 : values.reduce((sum, value) => sum + value, 0) / values.length;
}

function formatPercent(value) {
  return `${(value * 100).toFixed(2)}%`;
}

try {
  const references = await loadNotes(referencePath, 'reference');
  const predictions = await loadNotes(predictionPath, 'prediction');
  const matches = matchNotes(references, predictions);

  const truePositive = matches.length;
  const falsePositive = predictions.length - truePositive;
  const falseNegative = references.length - truePositive;
  const precision = predictions.length === 0 ? 0 : truePositive / predictions.length;
  const recall = references.length === 0 ? 0 : truePositive / references.length;
  const f1 = precision + recall === 0 ? 0 : (2 * precision * recall) / (precision + recall);
  const onsetMaeMs = mean(matches.map((match) => match.onsetError)) * 1000;
  const meanOverlap = mean(matches.map((match) => match.overlap));

  console.log(JSON.stringify({
    referenceNotes: references.length,
    predictedNotes: predictions.length,
    truePositive,
    falsePositive,
    falseNegative,
    precision,
    recall,
    f1,
    onsetMaeMs,
    meanDurationIoU: meanOverlap,
  }, null, 2));

  console.log(`\nPrecision ${formatPercent(precision)} | Recall ${formatPercent(recall)} | F1 ${formatPercent(f1)}`);
  console.log(`Onset MAE ${onsetMaeMs.toFixed(2)} ms | Duration IoU ${formatPercent(meanOverlap)}`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}
