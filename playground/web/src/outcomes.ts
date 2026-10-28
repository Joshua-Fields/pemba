// The assertions editor's per-line results (T6.6): ✓, ✗ or – in a gutter, and each failure
// marked inline with the runner's message. What a line shows comes from `place`
// (assertions.ts); nothing here decides it.

import { setDiagnostics, type Diagnostic } from '@codemirror/lint';
import { RangeSet, StateEffect, StateField, type Extension } from '@codemirror/state';
import { EditorView, gutter, GutterMarker } from '@codemirror/view';

import type { Outcome } from './assertions';

const SYMBOL: Record<Outcome['state'], string> = { pass: '✓', fail: '✗', skipped: '–' };
const LABEL: Record<Outcome['state'], string> = { pass: 'passed', fail: 'failed', skipped: 'not checked' };

class Marker extends GutterMarker {
  constructor(
    readonly state: Outcome['state'],
    readonly title: string,
  ) {
    super();
  }

  override eq(other: GutterMarker): boolean {
    return other instanceof Marker && other.state === this.state && other.title === this.title;
  }

  override toDOM(): Node {
    const span = document.createElement('span');
    span.className = `assert-${this.state}`;
    span.textContent = SYMBOL[this.state];
    span.title = this.title;
    span.setAttribute('aria-label', this.title);
    return span;
  }
}

const setMarkers = StateEffect.define<RangeSet<GutterMarker>>();

const markers = StateField.define<RangeSet<GutterMarker>>({
  create: () => RangeSet.empty,
  update(value, tr) {
    for (const e of tr.effects) if (e.is(setMarkers)) return e.value;
    return value.map(tr.changes);
  },
});

/** The gutter that shows each line's outcome. */
export const outcomeGutter: Extension = [
  markers,
  gutter({ class: 'cm-assert-gutter', markers: (v) => v.state.field(markers) }),
];

/** Shows `outcomes` in `view`: a gutter mark per line, and an inline mark per failure. */
export function showOutcomes(view: EditorView, outcomes: Outcome[]): void {
  const doc = view.state.doc;
  const length = doc.length;
  const ranges = [];
  const diagnostics: Diagnostic[] = [];
  for (const o of outcomes) {
    if (o.line > doc.lines) continue;
    const title = [LABEL[o.state], ...o.messages].join(': ');
    ranges.push(new Marker(o.state, title).range(doc.line(o.line).from));
    if (o.state === 'fail') {
      const from = Math.min(o.from, length);
      diagnostics.push({
        from,
        to: Math.min(Math.max(o.to, from), length),
        severity: 'error',
        message: o.messages.join('\n'),
      });
    }
  }
  view.dispatch(setDiagnostics(view.state, diagnostics), {
    effects: setMarkers.of(RangeSet.of(ranges, true)),
  });
}
