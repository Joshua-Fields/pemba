// The explain tree (docs/11-playground.md §4, T6.7): a check's trace JSON (`docs/06-api.md`
// §3.1) as a collapsible tree. Each node's line says what `CheckTrace::render` says for it
// (`crates/pemba-core/src/trace/render.rs`; Vitest holds the two to the T4.1 snapshots),
// so the tree and "copy as text" never disagree. Per-node durations are not shown: browser
// timers are too coarse to mean anything (§1.2).

export type Kind =
  | 'relation'
  | 'direct'
  | 'wildcard'
  | 'subject_set'
  | 'self'
  | 'computed'
  | 'union'
  | 'intersection'
  | 'exclusion'
  | 'arrow_any'
  | 'arrow_all'
  | 'undefined';

export type Source = 'evaluated' | 'memo_hit' | 'cycle' | 'cycle_hit' | 'skipped';

export interface TraceNode {
  id: number;
  node: string;
  kind: Kind;
  expr: string | null;
  role: 'base' | 'subtracted' | null;
  result: boolean | null;
  source: Source;
  ref: number | null;
  passes: number | null;
  provisional: boolean;
  tuple: string | null;
  truncated: boolean;
  children: TraceNode[];
}

export interface CheckTrace {
  resource: string;
  permission: string;
  subject: string;
  allowed: boolean;
  revision: number;
  truncated: boolean;
  root: TraceNode;
}

// ---------------------------------------------------------------------------------------
// Reading the JSON
// ---------------------------------------------------------------------------------------

const KINDS = new Set<string>([
  'relation', 'direct', 'wildcard', 'subject_set', 'self', 'computed', 'union',
  'intersection', 'exclusion', 'arrow_any', 'arrow_all', 'undefined',
]);
const SOURCES = new Set<string>(['evaluated', 'memo_hit', 'cycle', 'cycle_hit', 'skipped']);

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function isCount(v: unknown): v is number {
  return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
}

function stringOrNull(v: unknown): v is string | null {
  return v === null || typeof v === 'string';
}

function countOrNull(v: unknown): v is number | null {
  return v === null || isCount(v);
}

function node(v: unknown): TraceNode | null {
  if (!isObject(v)) return null;
  const { id, kind, expr, role, result, source, ref, passes, provisional, tuple, truncated, children } = v;
  if (
    !isCount(id) ||
    typeof v['node'] !== 'string' ||
    typeof kind !== 'string' ||
    !KINDS.has(kind) ||
    !stringOrNull(expr) ||
    !(role === null || role === 'base' || role === 'subtracted') ||
    !(result === null || typeof result === 'boolean') ||
    typeof source !== 'string' ||
    !SOURCES.has(source) ||
    // `result` is null exactly when the node was skipped (`04` §3.8).
    (result === null) !== (source === 'skipped') ||
    !countOrNull(ref) ||
    !countOrNull(passes) ||
    typeof provisional !== 'boolean' ||
    !stringOrNull(tuple) ||
    typeof truncated !== 'boolean' ||
    !Array.isArray(children)
  ) {
    return null;
  }
  const kids: TraceNode[] = [];
  for (const c of children) {
    const k = node(c);
    if (!k) return null;
    kids.push(k);
  }
  return {
    id,
    node: v['node'],
    kind: kind as Kind,
    expr,
    role,
    result,
    source: source as Source,
    ref,
    passes,
    provisional,
    tuple,
    truncated,
    children: kids,
  };
}

/** The trace in `text` (the `pemba check --json` format), or null if it is not one. */
export function readTrace(text: string): CheckTrace | null {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  if (!isObject(v) || v['format'] !== 'pemba.check_trace' || v['version'] !== 1) return null;
  const { resource, permission, subject, allowed, revision, truncated } = v;
  const root = node(v['root']);
  if (
    typeof resource !== 'string' ||
    typeof permission !== 'string' ||
    typeof subject !== 'string' ||
    typeof allowed !== 'boolean' ||
    !isCount(revision) ||
    typeof truncated !== 'boolean' ||
    !root ||
    root.result !== allowed
  ) {
    return null;
  }
  return { resource, permission, subject, allowed, revision, truncated, root };
}

// ---------------------------------------------------------------------------------------
// The lines, as `render()` writes them
// ---------------------------------------------------------------------------------------

/** Where a row stands relative to the granting path (the nodes that decided the answer). */
export type PathState = 'on' | 'leaves' | 'off';

/** A piece of a row's text; `ref` makes it a link to the node with that id. */
export interface Piece {
  text: string;
  ref?: number;
}

/** One line of the tree. `node` is null for the "not recorded" line of a truncated node. */
export interface Row {
  node: TraceNode | null;
  /** Ids this row stands for: the node, and the member a subject set is drawn as. */
  ids: number[];
  glyph: string;
  pieces: Piece[];
  path: PathState;
  /** Shows no evaluation of its own: skipped, or a memo reference. */
  reference: boolean;
  children: Row[];
}

/** The text of a row as `render()` writes it, without the branch drawing. */
export function rowText(r: Row): string {
  const text = r.pieces.map((p) => p.text).join('');
  return r.glyph === '' ? text : `${r.glyph} ${text}`;
}

const OPERATORS = new Set<Kind>(['computed', 'union', 'intersection', 'exclusion', 'arrow_any', 'arrow_all']);

/** A subject set is drawn as the member it names, with the stored tuple it came through. */
function shown(n: TraceNode): [TraceNode, string | null] {
  const only = n.children.length === 1 ? n.children[0] : undefined;
  if (n.kind === 'subject_set' && only) return [only, n.tuple];
  return [n, n.kind === 'subject_set' ? n.tuple : null];
}

/** A relation's point queries are folded into its line (the match) or left out (a miss). */
function drawn(parent: TraceNode, c: TraceNode): boolean {
  return !(parent.kind === 'relation' && (c.kind === 'direct' || c.kind === 'wildcard'));
}

function matchedTuple(n: TraceNode): string | null {
  if (n.kind !== 'relation') return null;
  const hit = n.children.find((c) => (c.kind === 'direct' || c.kind === 'wildcard') && c.result === true);
  return hit?.tuple ?? null;
}

function label(n: TraceNode, parent: TraceNode | null): string {
  const operand =
    (n.source === 'evaluated' || n.source === 'skipped') &&
    OPERATORS.has(n.kind) &&
    parent !== null &&
    parent.node === n.node &&
    OPERATORS.has(parent.kind);
  if (n.expr === null) return n.node;
  return operand ? n.expr : `${n.node} = ${n.expr}`;
}

/** The notes after a node: where its value came from, and why it may say little. */
function suffixes(n: TraceNode, s: TraceNode): Piece[][] {
  const out: Piece[][] = [];
  const refer = (of: number | null, what: string): Piece[] => {
    if (of === null) {
      return [{ text: what === '' ? 'not recorded: trace node limit reached' : `${what}unresolved: that node is not in this trace` }];
    }
    const link = { text: of < s.id ? 'see above' : 'see below', ref: of };
    return what === '' ? [link] : [{ text: what }, link];
  };
  switch (s.source) {
    case 'evaluated':
      break;
    case 'memo_hit':
      out.push(refer(s.ref, ''));
      break;
    case 'cycle': {
      const passes = s.passes ?? 0;
      out.push([{ text: `cycle: settled after ${passes} pass${passes === 1 ? '' : 'es'}` }]);
      break;
    }
    case 'cycle_hit':
      out.push(refer(s.ref, 'cycle: reached again, '));
      break;
    case 'skipped':
      out.push([{ text: 'skipped' }]);
      break;
  }
  const evaluatedFalse = s.result === false && s.source === 'evaluated';
  if (s.kind === 'self') {
    out.push([{ text: 'the subject itself' }]);
  } else if (s.kind === 'undefined') {
    out.push([{ text: 'no such member: contributes nothing' }]);
  } else if (s.kind === 'relation' && evaluatedFalse && !s.children.some((c) => c.kind === 'subject_set')) {
    out.push([{ text: 'no matching relationship' }]);
  } else if ((s.kind === 'arrow_any' || s.kind === 'arrow_all') && evaluatedFalse && s.children.length === 0) {
    out.push([{ text: 'no related objects' }]);
  }
  if (s.provisional || n.provisional) {
    out.push([{ text: 'provisional: computed from a cycle assumption that did not hold' }]);
  }
  return out;
}

/** The ids of the children that decided `n`'s value. */
function decisive(n: TraceNode): number[] {
  const evaluated = n.children.filter((c) => c.source !== 'skipped');
  const first = (want: boolean) => {
    const c = evaluated.find((x) => x.result === want);
    return c ? [c.id] : [];
  };
  const all = () => evaluated.map((c) => c.id);
  if (n.result === null) return [];
  switch (n.kind) {
    case 'relation':
    case 'union':
    case 'arrow_any':
      if (n.result) return first(true);
      break;
    case 'intersection':
    case 'arrow_all':
      if (!n.result) return first(false);
      break;
    case 'exclusion':
      if (!n.result) {
        const [base, sub] = evaluated;
        if (evaluated.length === 2 && base && sub && base.result === true) return [base.id, sub.id];
        if (base && evaluated.length <= 2) return [base.id];
        return all();
      }
      break;
    default:
      break;
  }
  return all();
}

function row(n: TraceNode, parent: TraceNode | null, path: PathState): Row {
  const [s, via] = shown(n);
  const pieces: Piece[] = [];
  if (n.role !== null) pieces.push({ text: n.role === 'base' ? 'base: ' : 'subtracted: ' });
  pieces.push({ text: label(s, parent) });
  if (via !== null) pieces.push({ text: `  via ${via}` });
  const t = matchedTuple(s);
  if (t !== null) pieces.push({ text: `  ← ${t}` });
  const notes = suffixes(n, s);
  if (path === 'leaves') notes.push([{ text: 'not needed' }]);
  if (notes.length > 0) {
    pieces.push({ text: '  (' });
    notes.forEach((note, i) => {
      if (i > 0) pieces.push({ text: '; ' });
      pieces.push(...note);
    });
    pieces.push({ text: ')' });
  }
  const glyph = n.result === true ? '✓' : n.result === false ? '✗' : '·';
  const reference = (x: TraceNode) => x.source === 'skipped' || x.source === 'memo_hit';
  const ids = s === n ? [n.id] : [n.id, s.id];
  return { node: n, ids, glyph, pieces, path, reference: reference(n) || reference(s), children: children(n, path) };
}

function children(n: TraceNode, path: PathState): Row[] {
  const [s] = shown(n);
  const decided = new Set(decisive(s));
  const out: Row[] = [];
  for (const c of s.children) {
    if (!drawn(s, c)) continue;
    let cpath: PathState;
    if (path !== 'on') cpath = 'off';
    else if (c.source === 'skipped') cpath = 'off';
    else cpath = decided.has(c.id) ? 'on' : 'leaves';
    out.push(row(c, s, cpath));
  }
  if (s.truncated) {
    out.push({
      node: null,
      ids: [],
      glyph: '',
      pieces: [{ text: '… (not recorded: trace node limit reached)' }],
      path: 'off',
      reference: true,
      children: [],
    });
  }
  return out;
}

/** The tree's rows, from the root. */
export function rows(trace: CheckTrace): Row {
  return row(trace.root, null, 'on');
}

// ---------------------------------------------------------------------------------------
// The DOM
// ---------------------------------------------------------------------------------------

const ANCHOR = 'trace-node-';

/** Opens every collapsed ancestor of `el`, then scrolls to it and highlights it alone. */
function reveal(root: HTMLElement, el: HTMLElement): void {
  for (const old of root.querySelectorAll('.trace-target')) old.classList.remove('trace-target');
  for (let p = el.parentElement; p; p = p.parentElement) {
    if (p instanceof HTMLDetailsElement) p.open = true;
  }
  el.scrollIntoView({ block: 'nearest' });
  el.classList.add('trace-target');
}

function rowElement(doc: Document, r: Row, root: HTMLElement): HTMLElement {
  const line = doc.createElement('span');
  line.className = `trace-row ${r.path}${r.reference ? ' reference' : ''}`;
  if (r.node) line.dataset['result'] = r.node.result === null ? 'skipped' : String(r.node.result);
  if (r.glyph !== '') {
    const g = doc.createElement('span');
    g.className = 'trace-glyph';
    g.textContent = r.glyph;
    line.append(g, ' ');
  }
  for (const p of r.pieces) {
    if (p.ref === undefined) {
      line.append(p.text);
      continue;
    }
    const a = doc.createElement('a');
    const target = `${ANCHOR}${p.ref}`;
    a.href = `#${target}`;
    a.className = 'trace-ref';
    a.textContent = p.text;
    a.addEventListener('click', (e) => {
      e.preventDefault();
      const el = root.querySelector<HTMLElement>(`[data-trace-ids~="${p.ref}"]`);
      if (el) reveal(root, el);
    });
    line.append(a);
  }
  return line;
}

function item(doc: Document, r: Row, root: HTMLElement): HTMLLIElement {
  const li = doc.createElement('li');
  if (r.ids.length > 0) {
    li.dataset['traceIds'] = r.ids.join(' ');
    const first = r.ids[0];
    if (first !== undefined) li.id = `${ANCHOR}${first}`;
  }
  const line = rowElement(doc, r, root);
  if (r.children.length === 0) {
    li.append(line);
    return li;
  }
  const details = doc.createElement('details');
  // The granting path is open; branches that did not decide the answer start closed.
  details.open = r.path === 'on';
  const summary = doc.createElement('summary');
  summary.append(line);
  const ul = doc.createElement('ul');
  for (const c of r.children) ul.append(item(doc, c, root));
  details.append(summary, ul);
  li.append(details);
  return li;
}

/**
 * The explain tree for `trace`, with a "copy as text" button that copies `text` (the
 * `CheckTrace::render` output `run` returned with it).
 */
export function traceTree(doc: Document, trace: CheckTrace, text: string): HTMLElement {
  const root = doc.createElement('div');
  root.className = 'trace';
  if (trace.truncated) {
    const banner = doc.createElement('p');
    banner.className = 'trace-banner';
    banner.textContent =
      'The trace reached its node limit (max_trace_nodes) and is incomplete. The answer is not: it is the full answer.';
    root.append(banner);
  }
  const tools = doc.createElement('div');
  tools.className = 'trace-tools';
  const title = doc.createElement('span');
  title.textContent = 'Explain';
  const status = doc.createElement('span');
  status.className = 'trace-copied';
  status.setAttribute('role', 'status');
  const copy = doc.createElement('button');
  copy.type = 'button';
  copy.className = 'trace-copy';
  copy.textContent = 'Copy as text';
  copy.addEventListener('click', () => {
    navigator.clipboard.writeText(text).then(
      () => (status.textContent = 'copied'),
      () => (status.textContent = 'copy failed: the browser did not allow it'),
    );
  });
  tools.append(title, copy, status);
  const tree = doc.createElement('ul');
  tree.className = 'trace-tree';
  tree.append(item(doc, rows(trace), root));
  root.append(tools, tree);
  return root;
}
