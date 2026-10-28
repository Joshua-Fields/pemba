import { describe as group, expect, it } from 'vitest';

import { MAX_EDITOR_BYTES, MAX_RELATIONSHIPS, checkLimits, describe } from './result';

group('describe', () => {
  it('shows a check answer with its revision', () => {
    expect(describe('check', '{"ok":true,"allowed":false,"revision":2,"schema_warnings":[]}')).toEqual({
      kind: 'check',
      allowed: false,
      revision: 2,
    });
  });

  it('shows lookups', () => {
    expect(describe('lookup_resources', '{"ok":true,"ids":["a","b"],"revision":3}')).toEqual({
      kind: 'lookup_resources',
      ids: ['a', 'b'],
      revision: 3,
    });
    expect(
      describe('lookup_subjects', '{"ok":true,"ids":[],"wildcard":true,"excluded":["x"],"revision":1}'),
    ).toEqual({ kind: 'lookup_subjects', ids: [], wildcard: true, excluded: ['x'], revision: 1 });
  });

  it('shows an engine error with its code, never as denied', () => {
    const v = describe(
      'check',
      '{"ok":false,"error":{"code":"MAX_DEPTH_EXCEEDED","stage":"query","message":"too deep"}}',
    );
    expect(v).toEqual({ kind: 'error', code: 'MAX_DEPTH_EXCEEDED', message: 'too deep', stage: 'query' });
  });

  // Fail closed: anything but the exact expected shape is an error, not an answer.
  it.each([
    ['not json', 'check'],
    ['null', 'check'],
    ['[]', 'check'],
    ['{"ok":true,"revision":1}', 'check'],
    ['{"ok":true,"allowed":"false","revision":1}', 'check'],
    ['{"ok":true,"allowed":false}', 'check'],
    ['{"ok":true,"allowed":false,"revision":-1}', 'check'],
    ['{"allowed":false,"revision":1}', 'check'],
    ['{"ok":false}', 'check'],
    ['{"ok":false,"error":{"message":"no code"}}', 'check'],
    ['{"ok":true,"ids":[1],"revision":1}', 'lookup_resources'],
    ['{"ok":true,"allowed":true,"revision":1}', 'lookup_resources'],
    ['{"ok":true,"ids":[],"revision":1}', 'lookup_subjects'],
  ] as const)('%s for %s is an error', (text, kind) => {
    expect(describe(kind, text).kind).toBe('error');
  });
});

group('checkLimits', () => {
  it('accepts normal input', () => {
    expect(checkLimits({ schema: 'definition user {}', relationships: 'doc:a#viewer@user:b' })).toBeNull();
  });

  it('caps each editor at 1 MB of UTF-8', () => {
    // Under the limit in UTF-16 units, over it in bytes.
    const text = '€'.repeat(MAX_EDITOR_BYTES / 3 + 1);
    const v = checkLimits({ schema: '', assertions: text });
    expect(v).toMatchObject({ kind: 'error', code: 'INPUT_TOO_LARGE', stage: 'assertions' });
  });

  it('caps relationships at 10 000, not counting blanks and comments', () => {
    const line = 'doc:a#viewer@user:b';
    const at = (n: number) => Array.from({ length: n }, () => line).join('\n');
    expect(checkLimits({ relationships: `${at(MAX_RELATIONSHIPS)}\n\n// note\n` })).toBeNull();
    expect(checkLimits({ relationships: at(MAX_RELATIONSHIPS + 1) })).toMatchObject({
      code: 'INPUT_TOO_LARGE',
      stage: 'relationships',
    });
  });
});
