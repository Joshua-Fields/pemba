// Shareable links (docs/11-playground.md §4): the page state as JSON, deflate-raw
// compressed by the browser's own `CompressionStream`, base64url in `#s=<…>`. The fragment
// never reaches a server. A link always names the engine that produced its answers, and a
// link that cannot be read in full loads nothing.

import type { QueryKind } from './types';

declare const __PEMBA_ENGINE__: string;

/** The engine this page runs: the pemba version, plus the short commit on `/next/`. */
export const ENGINE: string = __PEMBA_ENGINE__;

/** Everything a link restores. */
export interface State {
  v: 1;
  engine: string;
  schema: string;
  relationships: string;
  assertions: string;
  query: { kind: QueryKind; text: string };
}

/** Why a link could not be read. */
export class ShareError extends Error {}

/** The fragment key that holds the state. */
export const PREFIX = '#s=';

// Three full editors and some room: anything larger is not a state this page wrote.
const MAX_JSON_BYTES = 4_000_000;

const KINDS: readonly string[] = ['check', 'lookup_resources', 'lookup_subjects'];
const KEYS = ['assertions', 'engine', 'query', 'relationships', 'schema', 'v'];

function base64url(bytes: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function unbase64url(text: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(text) || text.length % 4 === 1) {
    throw new ShareError('the link is not base64url');
  }
  const binary = atob(text.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

async function pipe(bytes: Uint8Array, transform: CompressionStream | DecompressionStream, limit: number): Promise<Uint8Array> {
  const reader = new Blob([bytes as BlobPart]).stream().pipeThrough(transform).getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > limit) {
      await reader.cancel();
      throw new ShareError(`the link's state is larger than ${limit} bytes`);
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/** The fragment payload (`#s=` excluded) for `state`. */
export async function encodeState(state: State): Promise<string> {
  const json = new TextEncoder().encode(JSON.stringify(state));
  return base64url(await pipe(json, new CompressionStream('deflate-raw'), Infinity));
}

/** The state in a fragment payload; throws `ShareError` unless every part is there and well formed. */
export async function decodeState(payload: string): Promise<State> {
  const packed = unbase64url(payload);
  let json: Uint8Array;
  try {
    json = await pipe(packed, new DecompressionStream('deflate-raw'), MAX_JSON_BYTES);
  } catch (e) {
    if (e instanceof ShareError) throw e;
    throw new ShareError('the link is damaged (it does not decompress)');
  }
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(json));
  } catch {
    throw new ShareError('the link is damaged (it is not JSON)');
  }
  return readState(value);
}

function readState(v: unknown): State {
  if (typeof v !== 'object' || v === null || Array.isArray(v)) {
    throw new ShareError('the link holds no page state');
  }
  const o = v as Record<string, unknown>;
  if (o['v'] !== 1) throw new ShareError(`the link's format (v: ${JSON.stringify(o['v'])}) is not one this page reads`);
  const keys = Object.keys(o).sort();
  if (keys.length !== KEYS.length || keys.some((k, i) => k !== KEYS[i])) {
    throw new ShareError(`the link's state has keys ${keys.join(', ')}; expected ${KEYS.join(', ')}`);
  }
  for (const k of ['engine', 'schema', 'relationships', 'assertions']) {
    if (typeof o[k] !== 'string') throw new ShareError(`the link's ${k} is not text`);
  }
  const q = o['query'];
  if (
    typeof q !== 'object' || q === null || Array.isArray(q) ||
    Object.keys(q).length !== 2 ||
    typeof (q as Record<string, unknown>)['kind'] !== 'string' ||
    !KINDS.includes((q as Record<string, unknown>)['kind'] as string) ||
    typeof (q as Record<string, unknown>)['text'] !== 'string'
  ) {
    throw new ShareError("the link's query is not a query kind and text");
  }
  if (o['engine'] === '') throw new ShareError('the link does not name the engine that made it');
  return o as unknown as State;
}

/**
 * The release a link's engine names, when it can have a `/v/<version>/` page: a plain
 * version, not a `/next/` build (`version+commit`) or anything else.
 */
export function releaseOf(engine: string): string | null {
  return /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(engine) ? engine : null;
}

/** The site root for a page served under `base` (`/pemba/`, `/pemba/v/<version>/` or `/pemba/next/`). */
export function siteRoot(base: string): string {
  return base.replace(/(?:v\/[^/]+\/|next\/)$/, '');
}

/** The page of `release` on the site, with the same fragment. */
export function releaseUrl(base: string, release: string, fragment: string): string {
  return `${siteRoot(base)}v/${release}/${fragment}`;
}
