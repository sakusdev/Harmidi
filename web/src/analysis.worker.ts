/// <reference lib="webworker" />

import type { AnalysisResult, WorkerRequest, WorkerResponse } from './types';

type WasmModule = {
  default: () => Promise<unknown>;
  analyze_pcm: (
    samples: Float32Array,
    sampleRate: number,
    options: unknown,
  ) => AnalysisResult;
};

let wasmPromise: Promise<WasmModule> | null = null;

async function loadWasm(): Promise<WasmModule> {
  if (wasmPromise === null) {
    const modulePath = '/pkg/harmidi_core.js';
    wasmPromise = import(/* @vite-ignore */ modulePath).then(async (module: unknown) => {
      const wasm = module as WasmModule;
      await wasm.default();
      return wasm;
    });
  }
  return wasmPromise;
}

self.postMessage({ type: 'ready' } satisfies WorkerResponse);

self.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
  if (event.data.type !== 'analyze') return;

  void (async () => {
    try {
      const wasm = await loadWasm();
      const result = wasm.analyze_pcm(
        event.data.samples,
        event.data.sampleRate,
        event.data.options,
      );
      self.postMessage({ type: 'result', result } satisfies WorkerResponse);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      self.postMessage({ type: 'error', message } satisfies WorkerResponse);
    }
  })();
});
