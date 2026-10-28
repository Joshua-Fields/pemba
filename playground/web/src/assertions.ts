// Per-line results for the assertions editor (docs/11-playground.md §4, T6.6). The runner
// (`run_assertions`, `pemba test` semantics) reports only its failures, as (location,
// assertion text); this finds the line each names. A line is marked as passing only when
// the page can prove the runner checked it: every failure is placed on a line, and the
// lines the page counts as checked number exactly the runner's `checked`. Otherwise no
// line is marked as passing, and the summary still gives the runner's verdict.

/** One failure, as the runner reports it. */
export interface Failure {
  location: string;
  assertion: string;
  message: string;
}

/** A `run_assertions` report. */
export interface Report {
  passed: boolean;
  checked: number;
  failures: Failure[];
  warnings: string[];
  /** The 1-based line of the joined fixture where the assertions text starts. */
  assertionsLine: number;
}

/** A line of the assertions text the runner may report on. */
export interface Entry {
  /** 1-based. */
  line: number;
  /** UTF-16 offsets of the text the runner names (or of the key). */
  from: number;
  to: number;
  location: string;
  /** The text as the runner names it; empty for a whole key (`ops`, `expectErrors`). */
  assertion: string;
  /** The runner counts it in `checked`. */
  checked: boolean;
  /** `steps[i].` for an entry inside step `i`, else empty. */
  step: string;
}

/** One marked line. `skipped`: the step's write failed, so the runner did not check it. */
export interface Outcome {
  line: number;
  from: number;
  to: number;
  state: 'pass' | 'fail' | 'skipped';
  messages: string[];
}

export interface Placed {
  outcomes: Outcome[];
  /** Failures no line could be found for; the summary lists them. */
  unplaced: Failure[];
  /** Whether passing lines could be proved (see the header). */
  proved: boolean;
}

// ---------------------------------------------------------------------------------------
// Reading the report
// ---------------------------------------------------------------------------------------

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isCount(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
}

function isFailure(v: unknown): v is Failure {
  return (
    isObject(v) &&
    typeof v['location'] === 'string' &&
    typeof v['assertion'] === 'string' &&
    typeof v['message'] === 'string'
  );
}

/**
 * The report in `text`, the JSON `run_assertions` returned, or the error to show in its
 * place (with its code; never as a pass).
 */
export function readReport(text: string): Report | { code: string; message: string } {
  let answer: unknown;
  try {
    answer = JSON.parse(text);
  } catch {
    return { code: 'INTERNAL', message: 'the engine returned something that is not JSON' };
  }
  if (!isObject(answer)) return { code: 'INTERNAL', message: 'the engine returned no answer' };
  if (answer['ok'] === false) {
    const e = answer['error'];
    if (isObject(e) && typeof e['code'] === 'string' && typeof e['message'] === 'string') {
      return { code: e['code'], message: e['message'] };
    }
    return { code: 'INTERNAL', message: 'the engine failed without saying why' };
  }
  const { passed, checked, failures, warnings, assertions_line: line } = answer;
  if (
    answer['ok'] !== true ||
    typeof passed !== 'boolean' ||
    !isCount(checked) ||
    !Array.isArray(failures) ||
    !failures.every(isFailure) ||
    !Array.isArray(warnings) ||
    !warnings.every((w) => typeof w === 'string') ||
    !isCount(line) ||
    line < 1 ||
    // `passed` is "no failures"; a report that says otherwise is not one to trust.
    passed !== (failures.length === 0)
  ) {
    return { code: 'INTERNAL', message: `unexpected answer from the engine: ${text.slice(0, 200)}` };
  }
  return { passed, checked, failures, warnings, assertionsLine: line };
}

// ---------------------------------------------------------------------------------------
// Finding the lines
// ---------------------------------------------------------------------------------------

/** A key, or a sequence item (`key` null), and the column its children are deeper than. */
interface Frame {
  indent: number;
  key: string | null;
  items: number;
  /** For an item: its index in the parent's sequence. */
  index: number;
}

/** A scalar as written: its value and where it is in the line, or null if not a simple one. */
interface Scalar {
  value: string;
  start: number;
  end: number;
}

/**
 * The value of the scalar at `text[start..]`, up to a comment or the end of the line. Only
 * plain and single-line quoted scalars; anything else (flow, block, alias) is null.
 */
function scalar(text: string, start: number): Scalar | null {
  const rest = text.slice(start);
  const first = rest[0];
  if (first === undefined) return null;
  if (first === '"') {
    const m = /^"(?:[^"\\]|\\.)*"/.exec(rest);
    if (!m) return null;
    try {
      const value: unknown = JSON.parse(m[0]);
      return typeof value === 'string' ? { value, start, end: start + m[0].length } : null;
    } catch {
      return null;
    }
  }
  if (first === "'") {
    const m = /^'(?:[^']|'')*'/.exec(rest);
    if (!m) return null;
    return { value: m[0].slice(1, -1).replaceAll("''", "'"), start, end: start + m[0].length };
  }
  if ('[]{}|>&*!%@`,'.includes(first)) return null;
  // A plain scalar ends at a comment, which needs a space before its `#`.
  const comment = rest.search(/\s#/);
  const value = (comment === -1 ? rest : rest.slice(0, comment)).trimEnd();
  return value === '' ? null : { value, start, end: start + value.length };
}

/** `key: value` at `text[start..]`: the key and where its value starts, or null. */
function keyAt(text: string, start: number): { key: Scalar; value: number } | null {
  const rest = text.slice(start);
  let keyEnd: number;
  let key: string;
  if (rest.startsWith('"') || rest.startsWith("'")) {
    const s = scalar(text, start);
    if (!s || text[s.end] !== ':') return null;
    keyEnd = s.end;
    key = s.value;
  } else {
    // A plain key ends at the first `:` followed by a space or the end of the line.
    const m = /:(\s|$)/.exec(rest);
    if (!m || rest.startsWith('#') || rest.startsWith('- ')) return null;
    keyEnd = start + m.index;
    key = text.slice(start, keyEnd).trimEnd();
    if (key === '' || '[]{}|>&*!%@`,'.includes(key[0] ?? '')) return null;
  }
  let value = keyEnd + 1;
  while (text[value] === ' ' || text[value] === '\t') value++;
  return { key: { value: key, start, end: start + key.length }, value };
}

/**
 * The lines of `text` (fixture YAML minus `schema` and `relationships`) that the runner can
 * name: every check in `assertTrue`, `assertFalse` and `expectCheckErrors`, every
 * `expectedLookups` query, `expectErrors`, and in each step `expectError`, `ops` and its ops.
 */
export function entries(text: string): Entry[] {
  const out: Entry[] = [];
  const stack: Frame[] = [];
  let offset = 0;
  let block = -1; // inside a block scalar of a key at this indent
  const lines = text.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i] ?? '';
    const lineStart = offset;
    offset += raw.length + 1;
    const line = raw.replace(/\r$/, '');
    const indent = line.length - line.trimStart().length;
    const body = line.trim();
    if (block !== -1) {
      if (body === '' || indent > block) continue;
      block = -1;
    }
    if (body === '' || body.startsWith('#') || body === '---' || body === '...') continue;

    let col = indent;
    // Sequence items, `- ` (nested `- - x` is not a form fixtures use; it marks nothing).
    let item = false;
    if (line.startsWith('- ', col) || line.slice(col) === '-') {
      while (stack.length > 0 && (stack.at(-1)?.indent ?? -1) > col) stack.pop();
      const parent = stack.at(-1);
      if (!parent || parent.key === null) continue;
      parent.items++;
      stack.push({ indent: col + 1, key: null, items: 0, index: parent.items - 1 });
      col += 1;
      while (line[col] === ' ') col++;
      item = true;
      if (col >= line.length) continue;
    }

    const kv = keyAt(line, col);
    if (!kv) {
      if (item) entryForItem(out, stack, line, col, i + 1, lineStart);
      continue;
    }
    while (stack.length > 0 && (stack.at(-1)?.indent ?? -1) >= col) stack.pop();
    const valueStart = kv.value;
    const valueText = line.slice(valueStart);
    if (valueText.startsWith('|') || valueText.startsWith('>')) block = col;
    entryForKey(out, stack, kv.key, line, valueStart, i + 1, lineStart);
    stack.push({ indent: col, key: kv.key.value, items: 0, index: 0 });
  }
  return out;
}

/** The key path of `stack`: `step` is `steps[i].` inside step `i`; `keys` are the rest. */
function path(stack: Frame[]): { step: string; keys: string[]; inItem: boolean } {
  const keys: string[] = [];
  let step = '';
  for (let i = 0; i < stack.length; i++) {
    const f = stack[i];
    if (!f) continue;
    if (f.key !== null) {
      keys.push(f.key);
    } else if (i === 1 && stack[0]?.key === 'steps') {
      step = `steps[${f.index}].`;
      keys.length = 0;
    }
  }
  return { step, keys, inItem: stack.at(-1)?.key === null };
}

function push(
  out: Entry[],
  line: number,
  lineStart: number,
  s: { start: number; end: number },
  location: string,
  assertion: string,
  checked: boolean,
  step: string,
): void {
  out.push({ line, from: lineStart + s.start, to: lineStart + s.end, location, assertion, checked, step });
}

function entryForItem(out: Entry[], stack: Frame[], line: string, col: number, n: number, at: number): void {
  const { step, keys } = path(stack);
  const s = scalar(line, col);
  if (!s) return;
  const [a, b] = keys;
  if (keys.length === 2 && a === 'assertions' && (b === 'assertTrue' || b === 'assertFalse')) {
    push(out, n, at, s, `${step}${b}`, s.value, true, step);
  }
}

function entryForKey(
  out: Entry[],
  stack: Frame[],
  key: Scalar,
  line: string,
  valueStart: number,
  n: number,
  at: number,
): void {
  const { step, keys, inItem } = path(stack);
  const value = scalar(line, valueStart);
  const k = key.value;
  const top = stack.length === 0;
  if (keys.length === 1 && keys[0] === 'expectCheckErrors' && !inItem) {
    push(out, n, at, key, `${step}expectCheckErrors`, k, true, step);
  } else if (
    step === '' &&
    inItem &&
    k === 'query' &&
    keys.length === 2 &&
    keys[0] === 'expectedLookups' &&
    (keys[1] === 'resources' || keys[1] === 'subjects')
  ) {
    if (value) push(out, n, at, value, `expectedLookups.${keys[1]}`, value.value, true, step);
  } else if (top && k === 'expectErrors') {
    push(out, n, at, key, 'expectErrors', '', true, step);
  } else if (step !== '' && keys.length === 0 && k === 'expectError') {
    if (value) push(out, n, at, value, `${step}expectError`, value.value, true, step);
  } else if (step !== '' && keys.length === 0 && k === 'ops') {
    push(out, n, at, key, `${step}ops`, '', false, step);
  } else if (step !== '' && inItem && keys.length === 1 && keys[0] === 'ops') {
    if (value && (k === 'create' || k === 'touch' || k === 'delete')) {
      push(out, n, at, value, `${step}ops`, value.value, false, step);
    }
  }
}

// ---------------------------------------------------------------------------------------
// Placing the report
// ---------------------------------------------------------------------------------------

/** Failures that stop the whole fixture: nothing after them ran. */
const FATAL = new Set(['fixture', 'engine', 'schema', 'relationships']);

/** A runner position (`line 9 column 3`) in the joined fixture, as a line of the editor. */
const POSITION = /line (\d+) column (\d+)/;

/**
 * The outcome of every line of `text` the runner can name, given its `report`. A `fixture`
 * failure that names a position in the assertions text is placed there, with the message's
 * first line and the position counted in the editor's lines.
 */
export function place(text: string, report: Report): Placed {
  const found = entries(text);
  const lineCount = text.split('\n').length;
  const lineStarts: number[] = [0];
  for (let i = 0; i < text.length; i++) if (text[i] === '\n') lineStarts.push(i + 1);

  const byLine = new Map<number, Outcome>();
  const outcome = (e: { line: number; from: number; to: number }): Outcome => {
    let o = byLine.get(e.line);
    if (!o) {
      o = { line: e.line, from: e.from, to: e.to, state: 'pass', messages: [] };
      byLine.set(e.line, o);
    }
    return o;
  };

  let fatal = false;
  // Steps that stopped early, and where: `ops` before anything is counted, `expectError`
  // after counting itself.
  const blocked = new Map<string, 'ops' | 'expectError'>();
  const unplaced: Failure[] = [];
  for (const f of report.failures) {
    if (FATAL.has(f.location)) fatal = true;
    const step = /^steps\[\d+\]\./.exec(f.location)?.[0];
    // A step whose write failed (or wrongly succeeded) skips its assertions.
    if (step && f.location === `${step}ops`) {
      blocked.set(step, 'ops');
    } else if (
      step &&
      f.location === `${step}expectError` &&
      f.message.startsWith('expected the write to fail') &&
      !blocked.has(step)
    ) {
      blocked.set(step, 'expectError');
    }
    const hits = found.filter((e) => e.location === f.location && e.assertion === f.assertion);
    if (hits.length > 0) {
      for (const e of hits) {
        const o = outcome(e);
        o.state = 'fail';
        o.messages.push(f.message);
      }
      continue;
    }
    const pos = f.location === 'fixture' ? POSITION.exec(f.message) : null;
    const line = pos ? Number(pos[1]) - report.assertionsLine + 1 : 0;
    if (pos && line >= 1 && line <= lineCount) {
      const from = (lineStarts[line - 1] ?? 0) + Math.max(Number(pos[2]) - 1, 0);
      const next = lineStarts[line];
      const end = next === undefined ? text.length : next - 1;
      const first = (f.message.split('\n')[0] ?? '').replace(POSITION, `line ${line} column ${pos[2]}`);
      const o = outcome({ line, from: Math.min(from, end), to: end });
      o.state = 'fail';
      o.messages.push(first);
      continue;
    }
    unplaced.push(f);
  }

  const ran = (e: Entry) =>
    !blocked.has(e.step) ||
    (blocked.get(e.step) === 'expectError' && e.location === `${e.step}expectError`);
  const counted = found.filter((e) => e.checked && ran(e));
  const proved = !fatal && unplaced.length === 0 && counted.length === report.checked;
  if (proved) {
    for (const e of counted) outcome(e);
  }
  for (const e of found) {
    if (e.checked && !ran(e) && !byLine.has(e.line)) {
      const o = outcome(e);
      o.state = 'skipped';
      o.messages.push("not checked: this step's write did not go as expected");
    }
  }
  const outcomes = [...byLine.values()].sort((a, b) => a.line - b.line);
  return { outcomes, unplaced, proved };
}
