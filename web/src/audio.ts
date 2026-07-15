const MAX_DURATION_SECONDS = 180;

export type DecodedAudio = {
  samples: Float32Array;
  sampleRate: number;
  durationSeconds: number;
  wasTrimmed: boolean;
};

export async function decodeAudioFile(file: File): Promise<DecodedAudio> {
  const encoded = await file.arrayBuffer();
  const context = new AudioContext();

  try {
    const audio = await context.decodeAudioData(encoded.slice(0));
    const sampleCount = Math.min(
      audio.length,
      Math.floor(audio.sampleRate * MAX_DURATION_SECONDS),
    );
    const mono = new Float32Array(sampleCount);

    for (let channelIndex = 0; channelIndex < audio.numberOfChannels; channelIndex += 1) {
      const channel = audio.getChannelData(channelIndex);
      for (let index = 0; index < sampleCount; index += 1) {
        mono[index] = (mono[index] ?? 0) + (channel[index] ?? 0) / audio.numberOfChannels;
      }
    }

    return {
      samples: mono,
      sampleRate: audio.sampleRate,
      durationSeconds: sampleCount / audio.sampleRate,
      wasTrimmed: sampleCount < audio.length,
    };
  } finally {
    await context.close();
  }
}
