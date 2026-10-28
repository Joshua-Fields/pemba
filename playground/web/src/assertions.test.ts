import { describe as group, expect, it } from 'vitest';

import corpus from '../e2e/corpus.json' with { type: 'json' };
import { entries, place, readReport, type Failure, type Report } from './assertions';

function report(checked: number, failures: Failure[] = [], assertionsLine = 10): Report {
  return { passed: failures.length === 0, checked, failures, warnings: [], assertionsLine };
}

const TEXT = `# top
assertions:
  assertTrue:
    - doc:a#view@user:x
    - "doc:b#view@user:y"   # quoted
  assertFalse:
  - 'doc:c#view@user:z'
expectCheckErrors:
  doc:deep#view@user:r: MAX_DEPTH_EXCEEDED
expectedLookups:
  resources:
    - query: "doc#view@user:x"
      expect: [a]
  subjects:
    - expect: { ids: [x] }
      query: doc:a#view@user
steps:
  - ops:
      - create: doc:d#viewer@user:w
    expectError: ALREADY_EXISTS
    assertions:
      assertTrue:
        - doc:d#view@user:w
    expectCheckErrors:
      doc:e#view@user:w: MAX_DEPTH_EXCEEDED
  - ops:
      - delete: doc:a#viewer@user:x
    assertions:
      assertFalse:
        - doc:a#view@user:x
`;

group('entries', () => {
  it('finds every line the runner can name', () => {
    const got = entries(TEXT).map(({ line, location, assertion, checked }) => [line, location, assertion, checked]);
    expect(got).toEqual([
      [4, 'assertTrue', 'doc:a#view@user:x', true],
      [5, 'assertTrue', 'doc:b#view@user:y', true],
      [7, 'assertFalse', 'doc:c#view@user:z', true],
      [9, 'expectCheckErrors', 'doc:deep#view@user:r', true],
      [12, 'expectedLookups.resources', 'doc#view@user:x', true],
      [16, 'expectedLookups.subjects', 'doc:a#view@user', true],
      [18, 'steps[0].ops', '', false],
      [19, 'steps[0].ops', 'doc:d#viewer@user:w', false],
      [20, 'steps[0].expectError', 'ALREADY_EXISTS', true],
      [23, 'steps[0].assertTrue', 'doc:d#view@user:w', true],
      [25, 'steps[0].expectCheckErrors', 'doc:e#view@user:w', true],
      [26, 'steps[1].ops', '', false],
      [27, 'steps[1].ops', 'doc:a#viewer@user:x', false],
      [30, 'steps[1].assertFalse', 'doc:a#view@user:x', true],
    ]);
  });

  it('gives the offsets of the named text', () => {
    const [first, quoted] = entries(TEXT);
    expect(first && TEXT.slice(first.from, first.to)).toBe('doc:a#view@user:x');
    expect(quoted && TEXT.slice(quoted.from, quoted.to)).toBe('"doc:b#view@user:y"');
  });

  it('skips block scalars, comments and flow sequences', () => {
    const text = 'assertions:\n  assertTrue: [doc:a#v@user:x]\n  # - doc:b#v@user:y\nnote: |\n  - doc:c#v@user:z\n';
    expect(entries(text)).toEqual([]);
  });

  it('counts what the runner counts, for every corpus fixture', () => {
    for (const c of corpus) {
      const counted = entries(c.assertions).filter((e) => e.checked).length;
      expect(counted, c.fixture).toBe(c.checked);
    }
  });
});

group('place', () => {
  it('marks every checked line as passing when the counts agree', () => {
    const p = place(TEXT, report(10));
    expect(p.proved).toBe(true);
    expect(p.outcomes.map((o) => [o.line, o.state])).toEqual(
      [4, 5, 7, 9, 12, 16, 20, 23, 25, 30].map((l) => [l, 'pass']),
    );
    expect(p.outcomes.length).toBe(10);
  });

  it('puts each failure on its line with the runner message', () => {
    const p = place(TEXT, report(10, [
      { location: 'assertFalse', assertion: 'doc:c#view@user:z', message: 'expected false, but engine says true' },
      { location: 'steps[1].assertFalse', assertion: 'doc:a#view@user:x', message: 'expected false, but engine says true' },
    ]));
    expect(p.proved).toBe(true);
    const fails = p.outcomes.filter((o) => o.state === 'fail');
    expect(fails.map((o) => [o.line, o.messages])).toEqual([
      [7, ['expected false, but engine says true']],
      [30, ['expected false, but engine says true']],
    ]);
  });

  it('marks nothing as passing when the counts disagree', () => {
    const p = place(TEXT, report(11));
    expect(p.proved).toBe(false);
    expect(p.outcomes).toEqual([]);
  });

  it('marks nothing as passing when a failure has no line', () => {
    const f = { location: 'assertTrue', assertion: 'doc:q#view@user:q', message: 'expected true' };
    const p = place(TEXT, report(10, [f]));
    expect(p.proved).toBe(false);
    expect(p.unplaced).toEqual([f]);
    expect(p.outcomes).toEqual([]);
  });

  it('skips the assertions of a step whose write failed', () => {
    const p = place(TEXT, report(7, [{ location: 'steps[0].ops', assertion: '', message: 'write failed: boom' }]));
    expect(p.proved).toBe(true);
    const states = Object.fromEntries(p.outcomes.map((o) => [o.line, o.state]));
    expect(states).toEqual({ 4: 'pass', 5: 'pass', 7: 'pass', 9: 'pass', 12: 'pass', 16: 'pass', 18: 'fail', 20: 'skipped', 23: 'skipped', 25: 'skipped', 30: 'pass' });
  });

  it('a wrong error code still checks the step; a write that succeeded does not', () => {
    const wrong = place(TEXT, report(10, [{ location: 'steps[0].expectError', assertion: 'ALREADY_EXISTS', message: 'write failed with X instead: y' }]));
    expect(wrong.proved).toBe(true);
    expect(wrong.outcomes.find((o) => o.line === 23)?.state).toBe('pass');
    const ok = place(TEXT, report(8, [{ location: 'steps[0].expectError', assertion: 'ALREADY_EXISTS', message: 'expected the write to fail, but it succeeded' }]));
    expect(ok.proved).toBe(true);
    expect(ok.outcomes.find((o) => o.line === 20)?.state).toBe('fail');
    expect(ok.outcomes.find((o) => o.line === 23)?.state).toBe('skipped');
  });

  it('places an unknown key at its line, counted in the editor', () => {
    const text = 'assertions:\n  assertTrue: []\n  assertTure: []\n';
    const message = 'not a valid fixture: error: line 14 column 3: unknown field `assertTure`, expected one of assertTrue\n  --> <input>:14:3';
    const p = place(text, report(0, [{ location: 'fixture', assertion: '', message }], 12));
    expect(p.proved).toBe(false);
    expect(p.unplaced).toEqual([]);
    expect(p.outcomes).toEqual([
      {
        line: 3,
        from: 31,
        to: 45,
        state: 'fail',
        messages: ['not a valid fixture: error: line 3 column 3: unknown field `assertTure`, expected one of assertTrue'],
      },
    ]);
  });

  it('a failure outside the assertions marks no line as passing', () => {
    const p = place(TEXT, report(0, [{ location: 'schema', assertion: '', message: 'bad schema' }]));
    expect(p.proved).toBe(false);
    expect(p.outcomes).toEqual([]);
  });
});

group('readReport', () => {
  const ok = { ok: true, passed: true, checked: 2, failures: [], warnings: [], assertions_line: 5 };

  it('reads a report', () => {
    expect(readReport(JSON.stringify(ok))).toEqual({ passed: true, checked: 2, failures: [], warnings: [], assertionsLine: 5 });
  });

  it('turns an engine error into its code', () => {
    const e = { ok: false, error: { code: 'BAD_REQUEST', stage: 'request', message: 'no' } };
    expect(readReport(JSON.stringify(e))).toEqual({ code: 'BAD_REQUEST', message: 'no' });
  });

  it('fails closed on anything else', () => {
    for (const bad of ['nope', '[]', JSON.stringify({ ...ok, passed: false }), JSON.stringify({ ...ok, checked: -1 }), JSON.stringify({ ...ok, assertions_line: undefined })]) {
      expect(readReport(bad)).toMatchObject({ code: 'INTERNAL' });
    }
  });
});
