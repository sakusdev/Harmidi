#!/usr/bin/env node

import { spawnSync } from 'node:child_process';

const result = spawnSync(
  process.execPath,
  [
    'tools/evaluate-notes.mjs',
    'tools/fixtures/reference.json',
    'tools/fixtures/prediction.json',
  ],
  { encoding: 'utf8' },
);

if (result.status !== 0) {
  process.stderr.write(result.stderr);
  process.exit(result.status ?? 1);
}

for (const expected of ['"truePositive": 3', '"falsePositive": 0', '"falseNegative": 0', '"f1": 1']) {
  if (!result.stdout.includes(expected)) {
    console.error(`Evaluator output did not contain ${expected}`);
    console.error(result.stdout);
    process.exit(1);
  }
}

console.log('Note evaluator self-test passed.');
