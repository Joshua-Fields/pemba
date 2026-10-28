// Inline marks from a `run` answer (docs/11-playground.md §4): the schema's errors and
// warnings, and the relationship line the engine rejected. Positions come from the engine
// in UTF-16, as CodeMirror counts them; nothing here re-checks the text.

import type { Diagnostic } from '@codemirror/lint';

/** Marks for the schema and relationships editors. */
export interface Marks {
  schema: Diagnostic[];
  relationships: Diagnostic[];
}

/** One compile diagnostic, as the WASM API writes it. */
interface Wire {
  code: string;
  message: string;
  note: string | null;
  help: string | null;
  start: number;
  end: number;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isOffset(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
}

function optional(v: unknown): v is string | null | undefined {
  return v === null || v === undefined || typeof v === 'string';
}

function wire(v: unknown): Wire | null {
  if (!isObject(v)) return null;
  const { code, message, note, help, start, end } = v;
  if (typeof code !== 'string' || typeof message !== 'string') return null;
  if (!isOffset(start) || !isOffset(end) || end < start) return null;
  if (!optional(note) || !optional(help)) return null;
  return { code, message, note: note ?? null, help: help ?? null, start, end };
}

/** The tooltip body: code, message, then the note and the fix-it hint. */
function render(code: string, message: string, note: string | null, help: string | null): () => Node {
  return () => {
    const root = document.createElement('div');
    root.className = 'diag';
    const head = document.createElement('div');
    const c = document.createElement('code');
    c.className = 'diag-code';
    c.textContent = code;
    head.append(c, ` ${message}`);
    root.append(head);
    for (const [cls, label, text] of [
      ['diag-note', 'note', note],
      ['diag-help', 'help', help],
    ] as const) {
      if (text === null) continue;
      const p = document.createElement('div');
      p.className = cls;
      p.textContent = `${label}: ${text}`;
      root.append(p);
    }
    return root;
  };
}

function mark(
  severity: Diagnostic['severity'],
  from: number,
  to: number,
  length: number,
  code: string,
  message: string,
  note: string | null = null,
  help: string | null = null,
): Diagnostic {
  // An answer for an older text must not throw; a newer run replaces it.
  const f = Math.min(from, length);
  return {
    from: f,
    to: Math.min(Math.max(to, f), length),
    severity,
    message: `${code}: ${message}`,
    renderMessage: render(code, message, note, help),
  };
}

function list(v: unknown, severity: Diagnostic['severity'], length: number): Diagnostic[] {
  if (!Array.isArray(v)) return [];
  return v
    .map(wire)
    .filter((d): d is Wire => d !== null)
    .map((d) => mark(severity, d.start, d.end, length, d.code, d.message, d.note, d.help));
}

/**
 * The marks for `text`, the JSON `run` returned for these editors. An answer that is not
 * the expected shape marks nothing; the result pane already reports it as an error.
 */
export function marks(text: string, editors: { schema: string; relationships: string }): Marks {
  const none: Marks = { schema: [], relationships: [] };
  let answer: unknown;
  try {
    answer = JSON.parse(text);
  } catch {
    return none;
  }
  if (!isObject(answer)) return none;
  const schemaLength = editors.schema.length;
  if (answer['ok'] === true) {
    return { schema: list(answer['schema_warnings'], 'warning', schemaLength), relationships: [] };
  }
  const e = answer['error'];
  if (!isObject(e) || typeof e['code'] !== 'string' || typeof e['message'] !== 'string') return none;
  switch (e['stage']) {
    case 'schema':
      return {
        schema: [
          ...list(e['diagnostics'], 'error', schemaLength),
          ...list(e['warnings'], 'warning', schemaLength),
        ],
        relationships: [],
      };
    case 'relationships': {
      const { start, end } = e;
      if (!isOffset(start) || !isOffset(end)) return none;
      const d = mark('error', start, end, editors.relationships.length, e['code'], e['message']);
      return { schema: [], relationships: [d] };
    }
    default:
      return none;
  }
}
