// @vitest-environment jsdom
import { describe as group, expect, it } from 'vitest';

// Written by `playground/wasm/tests/trace_snapshots.rs`: every query of the four T4.1
// trace snapshots, its trace JSON from `run`, and the snapshot's `render()` text.
import snapshots from './trace-snapshots.json' with { type: 'json' };
import { readTrace, traceTree, type CheckTrace } from './trace';

/** The render text's node lines, without the header and the branch drawing. */
function renderedNodes(text: string): string[] {
  const lines = text.split('\n').filter((l) => l !== '');
  return lines.slice(1).filter((l) => !l.startsWith('note: ')).map((l) => l.replace(/^(?:│ {3}| {4})*(?:├── |└── )?/u, ''));
}

function tree(t: CheckTrace, text = 'TEXT'): HTMLElement {
  const el = traceTree(document, t, text);
  document.body.replaceChildren(el);
  return el;
}

function read(v: unknown): CheckTrace {
  const t = readTrace(JSON.stringify(v));
  if (!t) throw new Error('not a trace');
  return t;
}

group('the explain tree', () => {
  it('has a case per snapshot query', () => {
    const fixtures = new Set(snapshots.map((s) => s.fixture));
    expect([...fixtures].sort()).toEqual(['all_arrow', 'exclusion_banned', 'google_docs', 'nested_groups_cycle']);
  });

  for (const s of snapshots) {
    it(`${s.fixture}: ${s.query} reads as its render() snapshot, node for node`, () => {
      const el = tree(read(s.trace));
      const shown = [...el.querySelectorAll('.trace-row')].map((r) => r.textContent);
      expect(shown).toEqual(renderedNodes(s.render));
    });
  }

  it('links a memo hit or cycle hit to the node shown in full, opening it', () => {
    const cycle = snapshots.find((s) => s.query === 'group:a#member@user:eve');
    if (!cycle) throw new Error('snapshot missing');
    const el = tree(read(cycle.trace));
    const links = [...el.querySelectorAll<HTMLAnchorElement>('a.trace-ref')];
    expect(links.map((a) => a.textContent)).toEqual(['see above']);
    // jsdom does no layout.
    Element.prototype.scrollIntoView = () => {};
    for (const d of el.querySelectorAll('details')) d.open = false;
    const link = links[0];
    if (!link) throw new Error('no link');
    const target = el.querySelector<HTMLElement>(link.getAttribute('href') ?? '');
    expect(target?.textContent).toContain('group:a#member');
    link.click();
    expect(target?.classList.contains('trace-target')).toBe(true);
    // The hit's own ancestors are opened too, so the link stays visible.
    expect([...el.querySelectorAll('details')].every((d) => d.open || !d.contains(target ?? null))).toBe(true);
  });

  it('shows cycle pass counts and hides durations', () => {
    const cycle = snapshots.find((s) => s.query === 'group:a#member@user:eve');
    if (!cycle) throw new Error('snapshot missing');
    const el = tree(read(cycle.trace));
    expect(el.textContent).toContain('cycle: settled after 1 pass');
    expect(el.textContent).not.toMatch(/\d+\s*(ns|µs|ms)\b/);
  });

  it('a truncated trace has a banner and a not-recorded line', () => {
    const leaf = (id: number) => ({
      id, node: `group:g${id}#member`, kind: 'relation', expr: null, role: null, result: false, source: 'evaluated',
      ref: null, passes: null, provisional: false, tuple: null, duration_ns: 0, truncated: false, children: [],
    });
    const t = read({
      format: 'pemba.check_trace', version: 1, resource: 'doc:a', permission: 'view', subject: 'user:b',
      allowed: false, revision: 1, truncated: true, stats: {},
      root: { ...leaf(0), node: 'doc:a#view', kind: 'union', expr: 'a + b', truncated: true, children: [leaf(1)] },
    });
    const el = tree(t);
    expect(el.querySelector('.trace-banner')?.textContent).toContain('node limit');
    expect([...el.querySelectorAll('.trace-row')].map((r) => r.textContent)).toEqual([
      '✗ doc:a#view = a + b',
      '✗ group:g1#member  (no matching relationship)',
      '… (not recorded: trace node limit reached)',
    ]);
  });

  it('copy as text copies the render() text it was given', async () => {
    const s = snapshots[0];
    if (!s) throw new Error('no snapshots');
    let copied = '';
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: (t: string) => ((copied = t), Promise.resolve()) },
      configurable: true,
    });
    const el = tree(read(s.trace), s.render);
    el.querySelector<HTMLButtonElement>('.trace-copy')?.click();
    await Promise.resolve();
    expect(copied).toBe(s.render);
  });
});
