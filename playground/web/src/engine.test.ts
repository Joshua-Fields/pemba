import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { EngineClient, EngineFailure, type Reply, type WorkerLike } from './engine';

/** A worker that records messages and answers only when told to. */
class FakeWorker implements WorkerLike {
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: ((event: ErrorEvent) => void) | null = null;
  sent: { id: number; request: string }[] = [];
  terminated = false;

  postMessage(message: unknown): void {
    this.sent.push(message as { id: number; request: string });
  }

  terminate(): void {
    this.terminated = true;
  }

  reply(r: Reply): void {
    this.onmessage?.({ data: r } as MessageEvent);
  }
}

describe('EngineClient', () => {
  let workers: FakeWorker[];
  let client: EngineClient;

  beforeEach(() => {
    vi.useFakeTimers();
    workers = [];
    client = new EngineClient(() => {
      const w = new FakeWorker();
      workers.push(w);
      return w;
    }, 5000);
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('sends the request as JSON and resolves with the reply', async () => {
    const answer = client.run({ query: 'q' });
    const w = workers[0]!;
    expect(w.sent).toEqual([{ id: 0, request: '{"query":"q"}' }]);
    w.reply({ id: 0, out: '{"ok":true}' });
    await expect(answer).resolves.toBe('{"ok":true}');
  });

  it('stops a worker that does not answer within 5 s, then starts a new one', async () => {
    const answer = client.run({});
    const caught = answer.catch((e: unknown) => e);
    vi.advanceTimersByTime(4999);
    expect(workers[0]!.terminated).toBe(false);
    vi.advanceTimersByTime(1);
    const e = await caught;
    expect(e).toBeInstanceOf(EngineFailure);
    expect((e as EngineFailure).code).toBe('TIMEOUT');
    expect(workers[0]!.terminated).toBe(true);

    const next = client.run({});
    expect(workers).toHaveLength(2);
    workers[1]!.reply({ id: 1, out: 'ok' });
    await expect(next).resolves.toBe('ok');
  });

  it('a late reply from a stopped worker answers nothing', async () => {
    const first = client.run({}).catch((e: unknown) => e);
    vi.advanceTimersByTime(5000);
    await first;
    workers[0]!.reply({ id: 0, out: 'too late' });
    const second = client.run({});
    workers[1]!.reply({ id: 1, out: 'fresh' });
    await expect(second).resolves.toBe('fresh');
  });

  it('a trap fails every request in flight and replaces the worker', async () => {
    const a = client.run({}).catch((e: unknown) => e);
    const b = client.run({}).catch((e: unknown) => e);
    workers[0]!.reply({ id: 0, error: 'RuntimeError: unreachable' });
    for (const e of [await a, await b]) {
      expect((e as EngineFailure).code).toBe('ENGINE_CRASHED');
    }
    expect(workers[0]!.terminated).toBe(true);
    void client.run({});
    expect(workers).toHaveLength(2);
  });

  it('a worker that fails to load fails its requests', async () => {
    const a = client.run({}).catch((e: unknown) => e);
    workers[0]!.onerror?.({ message: 'no wasm', preventDefault() {} } as ErrorEvent);
    expect(((await a) as EngineFailure).code).toBe('ENGINE_CRASHED');
  });
});
