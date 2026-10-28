// The playground page (docs/11-playground.md §3, §4): three editors, the query bar, and the
// result of a fresh run 300 ms after the last edit.

import { basicSetup, EditorView } from 'codemirror';
import { yaml } from '@codemirror/lang-yaml';

import './style.css';
import { EngineClient, EngineFailure } from './engine';
import { checkLimits, describe, pageError } from './result';
import { follow, theme } from './theme';
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

let timer: ReturnType<typeof setTimeout> | undefined;
const schedule = () => {
  clearTimeout(timer);
  timer = setTimeout(() => void runNow(), DEBOUNCE_MS);
};

const onChange = EditorView.updateListener.of((u) => {
  if (u.docChanged) schedule();
});

function editor(id: string, doc: string, extra: ReturnType<typeof yaml>[] = []): EditorView {
  const view = new EditorView({
    doc,
    parent: element(id),
    extensions: [basicSetup, theme(), onChange, EditorView.lineWrapping, ...extra],
  });
  follow(view);
  return view;
}

const schema = editor('schema', SCHEMA);
const relationships = editor('relationships', RELATIONSHIPS);
const assertions = editor('assertions', ASSERTIONS, [yaml()]);

const client = new EngineClient(
  () => new Worker(new URL('./worker.ts', import.meta.url), { type: 'module' }),
);

// Only the newest run may paint: an older one can finish later.
let generation = 0;

async function runNow(): Promise<void> {
  const mine = ++generation;
  const editors = {
    schema: schema.state.doc.toString(),
    relationships: relationships.state.doc.toString(),
    assertions: assertions.state.doc.toString(),
  };
  const queryKind = kind.value as QueryKind;
  const request: RunRequest = {
    schema: editors.schema,
    relationships: editors.relationships,
    query: { kind: queryKind, text: text.value },
  };
  const limit = checkLimits(editors);
  let view: View;
  if (limit) {
    view = limit;
  } else {
    result.dataset['state'] = 'running';
    try {
      view = describe(queryKind, await client.run(request));
    } catch (e) {
      view =
        e instanceof EngineFailure
          ? pageError(e.code, e.message)
          : pageError('INTERNAL', String(e));
    }
  }
  if (mine === generation) render(view);
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
      break;
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
