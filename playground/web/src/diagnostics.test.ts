import { describe as group, expect, it } from 'vitest';

import { marks } from './diagnostics';

const editors = { schema: 'definition user {}\ndefinition doc { relation r: nope }', relationships: 'doc:a#r@user:b\nbad' };

function strip(list: ReturnType<typeof marks>['schema']) {
  return list.map(({ from, to, severity, message }) => ({ from, to, severity, message }));
}

group('marks', () => {
  it('marks schema errors and warnings with their code', () => {
    const answer = {
      ok: false,
      error: {
        code: 'SCHEMA_ERROR',
        stage: 'schema',
        message: '1 error',
        diagnostics: [
          { code: 'E_UNKNOWN_TYPE', message: 'unknown type `nope`', note: null, help: 'define it', line: 2, col: 30, start: 48, end: 52 },
        ],
        warnings: [{ code: 'W_DANGLING_DOC_COMMENT', message: 'dangling', note: null, help: null, start: 0, end: 3 }],
      },
    };
    const m = marks(JSON.stringify(answer), editors);
    expect(strip(m.schema)).toEqual([
      { from: 48, to: 52, severity: 'error', message: 'E_UNKNOWN_TYPE: unknown type `nope`' },
      { from: 0, to: 3, severity: 'warning', message: 'W_DANGLING_DOC_COMMENT: dangling' },
    ]);
    expect(m.relationships).toEqual([]);
  });

  it('marks warnings on a successful run', () => {
    const answer = {
      ok: true,
      revision: 1,
      allowed: true,
      schema_warnings: [{ code: 'W_X', message: 'w', note: null, help: null, start: 1, end: 2 }],
    };
    expect(strip(marks(JSON.stringify(answer), editors).schema)).toEqual([
      { from: 1, to: 2, severity: 'warning', message: 'W_X: w' },
    ]);
  });

  it('marks the relationship line the engine rejected', () => {
    for (const code of ['PARSE_ERROR', 'E_RELATION_NOT_FOUND']) {
      const answer = { ok: false, error: { code, stage: 'relationships', message: 'no', line: 2, start: 15, end: 18 } };
      const m = marks(JSON.stringify(answer), editors);
      expect(strip(m.relationships)).toEqual([{ from: 15, to: 18, severity: 'error', message: `${code}: no` }]);
      expect(m.schema).toEqual([]);
    }
  });

  it('clamps positions to the text, so an answer for older text cannot throw', () => {
    const answer = { ok: false, error: { code: 'E_X', stage: 'relationships', message: 'm', start: 90, end: 99 } };
    expect(strip(marks(JSON.stringify(answer), editors).relationships)).toEqual([
      { from: 18, to: 18, severity: 'error', message: 'E_X: m' },
    ]);
  });

  it('marks nothing for answers it cannot read, or errors with no place', () => {
    for (const text of [
      'not json',
      '[]',
      '{"ok":false}',
      '{"ok":false,"error":{"code":"E","message":"m","stage":"query","at":3}}',
      '{"ok":false,"error":{"code":"E","message":"m","stage":"relationships"}}',
      '{"ok":false,"error":{"code":"S","message":"m","stage":"schema","diagnostics":[{"code":"E","message":"m","start":5,"end":2}]}}',
    ]) {
      expect(marks(text, editors)).toEqual({ schema: [], relationships: [] });
    }
  });
});
