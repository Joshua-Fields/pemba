// The playground page (docs/11-playground.md §3, §4): three editors, the query bar, and the
// result of a fresh run 300 ms after the last edit, with its diagnostics marked inline, and
// the assertions run as a fixture (`pemba test` semantics) with a result per line.

import { basicSetup, EditorView } from 'codemirror';
import { yaml } from '@codemirror/lang-yaml';
import { lintGutter, setDiagnostics, type Diagnostic } from '@codemirror/lint';
import type { Extension, Text } from '@codemirror/state';

import './style.css';
import { place, readReport, type Placed, type Report } from './assertions';
import { marks } from './diagnostics';
import { dsl } from './dsl';
import { EngineClient, EngineFailure } from './engine';
import { outcomeGutter, showOutcomes } from './outcomes';
import { checkLimits, describe, pageError } from './result';
import { follow, theme } from './theme';
import { traceTree } from './trace';
import type { QueryKind, RunRequest, View } from './types';

const SCHEMA = `definition user {}

definition group {
    relation member: user | group#member
}

definition document {
    relation owner: user
    relation viewer: user | group#member
    permission edit = owner
    permission view = viewer + edit
}`;

const RELATIONSHIPS = `// Anne is in engineering; engineering can read the readme.
group:eng#member@user:anne
document:readme#viewer@group:eng#member
document:readme#owner@user:bob`;

const ASSERTIONS = `assertions:
  assertTrue:
    - document:readme#view@user:anne
  assertFalse:
    - document:readme#edit@user:anne
`;

const PLACEHOLDERS: Record<QueryKind, string> = {
  check: 'document:readme#view@user:anne',
  lookup_resources: 'document#view@user:anne',
  lookup_subjects: 'document:readme#view@user',
};

const DEBOUNCE_MS = 300;

function element<T extends HTMLElement>(id: string): T {
  const e = document.getElementById(id);
  if (!e) throw new Error(`#${id} is missing from the page`);
  return e as T;
}

const kind = element<HTMLSelectElement>('kind');
const text = element<HTMLInputElement>('text');
const result = element<HTMLDivElement>('result');
const assertionsResult = element<HTMLDivElement>('assertions-result');

let timer: ReturnType<typeof setTimeout> | undefined;
const schedule = () => {
  clearTimeout(timer);
  timer = setTimeout(() => void runNow(), DEBOUNCE_MS);
};

const onChange = EditorView.updateListener.of((u) => {
  if (u.docChanged) schedule();
});

function editor(id: string, doc: string, extra: Extension[] = []): EditorView {
  const view = new EditorView({
    doc,
    parent: element(id),
    extensions: [basicSetup, theme(), onChange, EditorView.lineWrapping, ...extra],
  });
  follow(view);
  return view;
}

const schema = editor('schema', SCHEMA, [dsl, lintGutter()]);
const relationships = editor('relationships', RELATIONSHIPS, [lintGutter()]);
const assertions = editor('assertions', ASSERTIONS, [yaml(), outcomeGutter]);

const client = new EngineClient(
  () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }),
);

// Only the newest run may paint: an older one can finish later.
let generation = 0;

/** Shows `diagnostics` in `view`, unless its text changed since the run (a newer run will). */
function annotate(view: EditorView, sent: Text, diagnostics: Diagnostic[]): void {
  if (view.state.doc === sent) view.dispatch(setDiagnostics(view.state, diagnostics));
}

/** The assertions pane: the runner's report and where its lines are, or why there is none. */
type Checked = { report: Report; placed: Placed } | { code: string; message: string };

async function runNow(): Promise<void> {
  const mine = ++generation;
  const sent = {
    schema: schema.state.doc,
    relationships: relationships.state.doc,
    assertions: assertions.state.doc,
  };
  const editors = {
    schema: schema.state.doc.toString(),
    relationships: relationships.state.doc.toString(),
    assertions: assertions.state.doc.toString(),
  };
  const queryKind = kind.value as QueryKind;
  const request: RunRequest = {
    schema: editors.schema,
    relationships: editors.relationships,
    // A check also returns its trace, for the explain tree.
    query: queryKind === 'check' ? { kind: queryKind, text: text.value, explain: true } : { kind: queryKind, text: text.value },
  };
  const limit = checkLimits(editors);
  let view: View;
  let checked: Checked;
  // Without an answer there is nothing to mark, so old marks are cleared, not kept.
  let found = { schema: [] as Diagnostic[], relationships: [] as Diagnostic[] };
  if (limit) {
    view = limit;
    checked = limit.kind === 'error' ? limit : { code: 'INTERNAL', message: 'no answer' };
  } else {
    result.dataset['state'] = 'running';
    assertionsResult.dataset['state'] = 'running';
    try {
      const [out, report] = await Promise.all([client.run(request), client.runAssertions(editors)]);
      view = describe(queryKind, out);
      found = marks(out, editors);
      const r = readReport(report);
      checked = 'code' in r ? r : { report: r, placed: place(editors.assertions, r) };
    } catch (e) {
      view =
        e instanceof EngineFailure
          ? pageError(e.code, e.message)
          : pageError('INTERNAL', String(e));
      checked = view.kind === 'error' ? view : { code: 'INTERNAL', message: String(e) };
    }
  }
  if (mine !== generation) return;
  render(view);
  annotate(schema, sent.schema, found.schema);
  annotate(relationships, sent.relationships, found.relationships);
  renderAssertions(checked);
  if (assertions.state.doc === sent.assertions) {
    showOutcomes(assertions, 'report' in checked ? checked.placed.outcomes : []);
  }
}

function renderAssertions(checked: Checked): void {
  const pane = assertionsResult;
  pane.replaceChildren();
  const line = (cls: string, content: string) => {
    const p = document.createElement('p');
    p.className = cls;
    p.textContent = content;
    pane.append(p);
  };
  if (!('report' in checked)) {
    pane.dataset['state'] = 'error';
    line('verdict error', `Error: ${checked.code}`);
    line('message', checked.message);
    return;
  }
  const { report, placed } = checked;
  pane.dataset['state'] = report.passed ? 'answer' : 'error';
  const n = report.checked === 1 ? '1 check' : `${report.checked} checks`;
  if (report.passed) {
    line('verdict allowed', `${n}, all passed`);
  } else {
    const f = report.failures.length === 1 ? '1 failure' : `${report.failures.length} failures`;
    line('verdict error', `${f} (${n})`);
    const ul = document.createElement('ul');
    ul.className = 'failures';
    for (const x of report.failures) {
      const li = document.createElement('li');
      const where = document.createElement('code');
      where.textContent = x.assertion === '' ? x.location : `${x.location} ${x.assertion}`;
      li.append(where, document.createElement('br'), x.message);
      ul.append(li);
    }
    pane.append(ul);
  }
  if (!placed.proved && report.checked > 0) {
    line('meta', 'Lines are not marked as passing: the page could not match every check the runner counted to a line.');
  }
  for (const w of report.warnings) line('meta', `warning: ${w}`);
}

function render(view: View): void {
  result.replaceChildren();
  result.dataset['state'] = view.kind === 'error' ? 'error' : 'answer';
  const line = (cls: string, content: string) => {
    const p = document.createElement('p');
    p.className = cls;
    p.textContent = content;
    result.append(p);
    return p;
  };
  const list = (ids: string[]) => {
    const ul = document.createElement('ul');
    ul.className = 'ids';
    for (const id of ids) {
      const li = document.createElement('li');
      li.textContent = id;
      ul.append(li);
    }
    result.append(ul);
  };
  switch (view.kind) {
    case 'error':
      line('verdict error', `Error: ${view.code}`);
      line('message', view.message);
      line('meta', `stage: ${view.stage}`);
      return;
    case 'check':
      line(`verdict ${view.allowed ? 'allowed' : 'denied'}`, view.allowed ? 'allowed' : 'denied');
      line('meta', `revision ${view.revision}`);
      if (view.explain) result.append(traceTree(document, view.explain.trace, view.explain.text));
      return;
    case 'lookup_resources':
      line('verdict', view.ids.length === 1 ? '1 resource' : `${view.ids.length} resources`);
      list(view.ids);
      break;
    case 'lookup_subjects':
      if (view.wildcard) {
        line('verdict', 'every subject of this type (wildcard)');
        if (view.excluded.length > 0) {
          line('meta', 'except:');
          list(view.excluded);
        }
      } else {
        line('verdict', view.ids.length === 1 ? '1 subject' : `${view.ids.length} subjects`);
        list(view.ids);
      }
      break;
  }
  line('meta', `revision ${view.revision}`);
}

function syncPlaceholder(): void {
  text.placeholder = PLACEHOLDERS[kind.value as QueryKind];
}

kind.addEventListener('change', () => {
  syncPlaceholder();
  text.value = PLACEHOLDERS[kind.value as QueryKind];
  void runNow();
});
text.addEventListener('input', schedule);
element<HTMLFormElement>('query').addEventListener('submit', (e) => {
  e.preventDefault();
  clearTimeout(timer);
  void runNow();
});

syncPlaceholder();
text.value = PLACEHOLDERS.check;
void runNow();
