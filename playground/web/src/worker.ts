// The engine, off the main thread (docs/11-playground.md §3): one `run` per message.
import init, { run } from './wasm/pemba_playground.js';
import type { Reply } from './engine';

const ready = init();

self.onmessage = async (event: MessageEvent<{ id: number; request: string }>) => {
  const { id, request } = event.data;
  let reply: Reply;
  try {
    await ready;
    reply = { id, out: run(request) };
  } catch (e) {
    reply = { id, error: String(e) };
  }
  self.postMessage(reply);
};
