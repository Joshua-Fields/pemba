// The page's side of the engine worker (docs/11-playground.md §3). A worker that does not
// answer within the timeout, or whose WASM traps, is terminated and replaced; its pending
// requests fail with `TIMEOUT` or `ENGINE_CRASHED`, never with an answer.

/** What the client needs of a `Worker`; tests pass a fake. */
export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  onmessage: ((event: MessageEvent) => void) | null;
  onerror: ((event: ErrorEvent) => void) | null;
}

/** The WASM API function a message calls. */
export type Op = 'run' | 'run_assertions' | 'parse_fixture';

/** A request to the worker. */
export interface Message {
  id: number;
  op: Op;
  request: string;
}

/** A worker reply: the function's output, or why there is none. */
export type Reply = { id: number; out: string } | { id: number; error: string };

/** A failure of the worker itself, with the code the page shows. */
export class EngineFailure extends Error {
  readonly code: 'TIMEOUT' | 'ENGINE_CRASHED';

  constructor(code: 'TIMEOUT' | 'ENGINE_CRASHED', message: string) {
    super(message);
    this.code = code;
  }
}

interface Pending {
  resolve: (out: string) => void;
  reject: (e: EngineFailure) => void;
  timer: ReturnType<typeof setTimeout>;
}

export const TIMEOUT_MS = 5000;

export class EngineClient {
  private worker: WorkerLike | null = null;
  private next = 0;
  private readonly pending = new Map<number, Pending>();
  private readonly spawn: () => WorkerLike;
  private readonly timeoutMs: number;

  constructor(spawn: () => WorkerLike, timeoutMs = TIMEOUT_MS) {
    this.spawn = spawn;
    this.timeoutMs = timeoutMs;
  }

  /** Sends one `run` request; resolves to the engine's JSON text. */
  run(request: unknown): Promise<string> {
    return this.call('run', JSON.stringify(request));
  }

  /** Sends one `run_assertions` request; resolves to the engine's JSON text. */
  runAssertions(request: unknown): Promise<string> {
    return this.call('run_assertions', JSON.stringify(request));
  }

  /** Splits fixture YAML into the three editors; resolves to the engine's JSON text. */
  parseFixture(source: string): Promise<string> {
    return this.call('parse_fixture', source);
  }

  private call(op: Op, request: string): Promise<string> {
    const worker = this.ensure();
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        const seconds = this.timeoutMs / 1000;
        this.fail(new EngineFailure('TIMEOUT', `the engine did not answer within ${seconds} s and was stopped`));
      }, this.timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      worker.postMessage({ id, op, request } satisfies Message);
    });
  }

  private ensure(): WorkerLike {
    if (this.worker) return this.worker;
    const w = this.spawn();
    w.onmessage = (event: MessageEvent) => this.reply(event.data as Reply);
    w.onerror = (event: ErrorEvent) => {
      event.preventDefault();
      this.fail(new EngineFailure('ENGINE_CRASHED', `the engine failed to load or crashed: ${event.message}`));
    };
    this.worker = w;
    return w;
  }

  private reply(r: Reply): void {
    const p = this.pending.get(r.id);
    if (!p) return;
    if ('out' in r) {
      clearTimeout(p.timer);
      this.pending.delete(r.id);
      p.resolve(r.out);
    } else {
      // A trapped WASM instance is not safe to call again: replace the worker.
      this.fail(new EngineFailure('ENGINE_CRASHED', `the engine crashed: ${r.error}`));
    }
  }

  /** Stops the worker and fails everything in flight. */
  private fail(e: EngineFailure): void {
    this.worker?.terminate();
    this.worker = null;
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(e);
    }
    this.pending.clear();
  }
}
