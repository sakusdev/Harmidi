import type { DetectedNote } from './types';

const TICKS_PER_BEAT = 480;

function pushU16(target: number[], value: number): void {
  target.push((value >>> 8) & 0xff, value & 0xff);
}

function pushU32(target: number[], value: number): void {
  target.push(
    (value >>> 24) & 0xff,
    (value >>> 16) & 0xff,
    (value >>> 8) & 0xff,
    value & 0xff,
  );
}

function pushAscii(target: number[], value: string): void {
  for (const character of value) target.push(character.charCodeAt(0));
}

function variableLength(value: number): number[] {
  let buffer = value & 0x7f;
  const bytes: number[] = [];

  while ((value >>= 7) > 0) {
    buffer <<= 8;
    buffer |= (value & 0x7f) | 0x80;
  }

  while (true) {
    bytes.push(buffer & 0xff);
    if ((buffer & 0x80) === 0) break;
    buffer >>= 8;
  }

  return bytes;
}

type MidiEvent = {
  tick: number;
  priority: number;
  bytes: number[];
};

export function createMidiFile(notes: DetectedNote[], bpm: number): Uint8Array {
  const safeBpm = Math.min(300, Math.max(20, bpm));
  const ticksPerSecond = (TICKS_PER_BEAT * safeBpm) / 60;
  const events: MidiEvent[] = [];

  const microsecondsPerQuarter = Math.round(60_000_000 / safeBpm);
  events.push({
    tick: 0,
    priority: 0,
    bytes: [
      0xff,
      0x51,
      0x03,
      (microsecondsPerQuarter >>> 16) & 0xff,
      (microsecondsPerQuarter >>> 8) & 0xff,
      microsecondsPerQuarter & 0xff,
    ],
  });

  for (const note of notes) {
    const startTick = Math.max(0, Math.round(note.startSeconds * ticksPerSecond));
    const endTick = Math.max(startTick + 1, Math.round(note.endSeconds * ticksPerSecond));
    const key = Math.min(127, Math.max(0, Math.round(note.midiNote)));
    const velocity = Math.min(127, Math.max(1, Math.round(note.velocity)));

    events.push({ tick: startTick, priority: 2, bytes: [0x90, key, velocity] });
    events.push({ tick: endTick, priority: 1, bytes: [0x80, key, 0] });
  }

  events.sort((left, right) => left.tick - right.tick || left.priority - right.priority);

  const track: number[] = [];
  let previousTick = 0;
  for (const event of events) {
    track.push(...variableLength(event.tick - previousTick), ...event.bytes);
    previousTick = event.tick;
  }
  track.push(0x00, 0xff, 0x2f, 0x00);

  const output: number[] = [];
  pushAscii(output, 'MThd');
  pushU32(output, 6);
  pushU16(output, 0);
  pushU16(output, 1);
  pushU16(output, TICKS_PER_BEAT);
  pushAscii(output, 'MTrk');
  pushU32(output, track.length);
  output.push(...track);

  return new Uint8Array(output);
}
