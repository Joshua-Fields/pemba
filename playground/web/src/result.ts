// Turns the engine's JSON into what the page shows. Fail closed (docs/11-playground.md §3):
// an error, or an answer that is not exactly the expected shape, is shown as an error with
// its code, never as "denied" or an empty list.

import type { QueryKind, View } from './types';

/** An error the page itself raises (not the engine's). */
export function pageError(code: string, message: string, stage = 'request'): View {
  return { kind: 'error', code, message, stage };
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isStrings(v: unknown): v is string[] {
  return Array.isArray(v) && v.every((x) => typeof x === 'string');
}

function isRevision(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
}

/** The view of `text`, the JSON `run` returned for a query of `kind`. */
export function describe(kind: QueryKind, text: string): View {
  let answer: unknown;
  try {
    answer = JSON.parse(text);
  } catch {
    return pageError('INTERNAL', 'the engine returned something that is not JSON');
  }
  if (!isObject(answer)) return pageError('INTERNAL', 'the engine returned no answer');
  if (answer['ok'] === false) {
    const e = answer['error'];
    if (isObject(e) && typeof e['code'] === 'string' && typeof e['message'] === 'string') {
      const stage = typeof e['stage'] === 'string' ? e['stage'] : 'request';
      return { kind: 'error', code: e['code'], message: e['message'], stage };
    }
    return pageError('INTERNAL', 'the engine failed without saying why');
  }
  const unexpected = pageError('INTERNAL', `unexpected answer from the engine: ${text.slice(0, 200)}`);
  if (answer['ok'] !== true || !isRevision(answer['revision'])) return unexpected;
  const revision = answer['revision'];
  switch (kind) {
    case 'check':
      return typeof answer['allowed'] === 'boolean'
        ? { kind, allowed: answer['allowed'], revision }
        : unexpected;
    case 'lookup_resources':
      return isStrings(answer['ids']) ? { kind, ids: answer['ids'], revision } : unexpected;
    case 'lookup_subjects': {
      const { ids, wildcard, excluded } = answer;
      return isStrings(ids) && typeof wildcard === 'boolean' && isStrings(excluded)
        ? { kind, ids, wildcard, excluded, revision }
        : unexpected;
    }
  }
}

/** Per-editor size cap (docs/11-playground.md §3): 1 MB of UTF-8. */
export const MAX_EDITOR_BYTES = 1_000_000;
/** Relationship lines (not blank, not `//` comments) the page accepts. */
export const MAX_RELATIONSHIPS = 10_000;

/** The first limit the editors break, as an error view, or `null`. */
export function checkLimits(editors: Record<string, string>): View | null {
  const encoder = new TextEncoder();
  for (const [name, text] of Object.entries(editors)) {
    const bytes = encoder.encode(text).length;
    if (bytes > MAX_EDITOR_BYTES) {
      return pageError(
        'INPUT_TOO_LARGE',
        `the ${name} editor holds ${bytes} bytes; the limit is ${MAX_EDITOR_BYTES}`,
        name,
      );
    }
  }
  const lines = (editors['relationships'] ?? '')
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l !== '' && !l.startsWith('//')).length;
  if (lines > MAX_RELATIONSHIPS) {
    return pageError(
      'INPUT_TOO_LARGE',
      `${lines} relationships; the limit is ${MAX_RELATIONSHIPS}`,
      'relationships',
    );
  }
  return null;
}
