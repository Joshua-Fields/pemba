// The engine, off the main thread (docs/11-playground.md §3): one API call per message.
import init, { run, run_assertions } from './wasm/pemba_playground.js';
import type { Message, Reply } from './engine';

const ready = init();
const ops = { run, run_assertions };

self.onmessage = async (event: MessageEvent<Message>) => {
  const { id, op, request } = event.data;
  let reply: Reply;
  try {
    await ready;
    reply = { id, out: ops[op](request) };
  } catch (e) {
    reply = { id, error: String(e) };
  }
  self.postMessage(reply);
};
