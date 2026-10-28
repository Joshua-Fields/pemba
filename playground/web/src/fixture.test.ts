/// <reference types="node" />
import { readFileSync } from 'node:fs';

import { beforeAll, describe, expect, it } from 'vitest';

// Every `testdata/fixtures` file split into the three editors, and how many checks the
// native runner counts in it (written by `playground/wasm/tests/assertion_corpus.rs`).
import corpus from '../e2e/corpus.json' with { type: 'json' };
import { EXAMPLES } from './examples';
import { exportFixture, type Parts } from './fixture';
// Built by `npm run wasm`, as before every `npm test` in CI.
import init, { parse_fixture, run, run_assertions } from './wasm/pemba_playground.js';

beforeAll(async () => {
  await init({ module_or_path: readFileSync(new URL('./wasm/pemba_playground_bg.wasm', import.meta.url)) });
});

/** Import, as the page does it: `parse_fixture` in the engine. */
function importFixture(yaml: string): Parts {
  const out = JSON.parse(parse_fixture(yaml)) as Record<string, unknown>;
  expect(out['ok'], JSON.stringify(out)).toBe(true);
  return { schema: out['schema'] as string, relationships: out['relationships'] as string, assertions: out['assertions'] as string };
}

const SCHEMA = 'definition user {}\ndefinition doc {\n    relation viewer: user\n    permission view = viewer\n}';

describe('export', () => {
  it('writes the layout the spec names', () => {
    const yaml = exportFixture({ schema: SCHEMA, relationships: 'doc:a#viewer@user:anne', assertions: 'assertions:\n  assertTrue:\n    - doc:a#view@user:anne\n' });
    expect(yaml).toBe(
      'schema: |-\n  definition user {}\n  definition doc {\n      relation viewer: user\n      permission view = viewer\n  }\n\n' +
        'relationships: |-\n  doc:a#viewer@user:anne\n' +
        'assertions:\n  assertTrue:\n    - doc:a#view@user:anne\n',
    );
  });

  // Text an editor can hold that a plain `|-` would change or reject.
  const awkward: [string, string][] = [
    ['empty', ''],
    ['one newline', '\n'],
    ['only newlines', '\n\n\n'],
    ['a trailing newline', 'definition user {}\n'],
    ['trailing blank lines', 'definition user {}\n\n\n'],
    ['a leading space', '  definition user {}'],
    ['a leading blank line', '\ndefinition user {}'],
    ['a leading line of spaces', '    \ndefinition user {}'],
    ['a trailing line of spaces', 'definition user {}\n   '],
    ['blank lines inside', 'definition user {}\n\n\ndefinition doc {}'],
    ['YAML syntax inside', '# not a comment\nkey: value\n---\n- item\n"quoted"'],
    ['CRLF', 'definition user {}\r\ndefinition doc {}'],
    ['astral characters', '// 👩‍💻 ✓\ndefinition user {}'],
  ];
  for (const [name, text] of awkward) {
    it(`keeps a schema and relationships with ${name}`, () => {
      const parts = { schema: text, relationships: text, assertions: '' };
      const back = importFixture(exportFixture(parts));
      const lf = text.replace(/\r\n/g, '\n');
      expect(back).toEqual({ schema: lf, relationships: lf, assertions: '' });
    });
  }

  it('ends the assertions with a newline so a later key cannot join them', () => {
    expect(exportFixture({ schema: '', relationships: '', assertions: 'assertions: {}' })).toBe(
      'schema: |-\n\nrelationships: |-\nassertions: {}\n',
    );
  });
});

describe('every corpus fixture survives export → import → run', () => {
  for (const c of corpus) {
    it(c.fixture, () => {
      const parts = { schema: c.schema, relationships: c.relationships, assertions: c.assertions };
      const yaml = exportFixture(parts);
      const back = importFixture(yaml);
      expect(back).toEqual(parts);
      // And the run is the same: everything passes, with the native runner's count.
      const report = JSON.parse(run_assertions(JSON.stringify(back))) as Record<string, unknown>;
      expect(report['ok'], JSON.stringify(report)).toBe(true);
      expect(report['failures']).toEqual([]);
      expect(report['passed']).toBe(true);
      expect(report['checked']).toBe(c.checked);
      // Exporting what came back gives the same file.
      expect(exportFixture(back)).toBe(yaml);
    });
  }
});

describe('the example menu', () => {
  it('offers the three examples the spec names', () => {
    expect(EXAMPLES.map((e) => e.name)).toEqual(['google_docs', 'github_like', 'agent_authz']);
  });

  for (const ex of EXAMPLES) {
    it(`${ex.name} imports, passes, and its query answers`, () => {
      const parts = importFixture(ex.source);
      const report = JSON.parse(run_assertions(JSON.stringify(parts))) as Record<string, unknown>;
      expect(report['passed'], JSON.stringify(report)).toBe(true);
      const answer = JSON.parse(run(JSON.stringify({ schema: parts.schema, relationships: parts.relationships, query: ex.query }))) as Record<string, unknown>;
      expect(answer['ok'], JSON.stringify(answer)).toBe(true);
    });
  }
});
