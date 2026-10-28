import { describe, expect, it } from 'vitest';

import corpus from '../e2e/corpus.json' with { type: 'json' };
import { decodeState, ENGINE, encodeState, releaseOf, releaseUrl, ShareError, siteRoot, type State } from './share';

const STATE: State = {
  v: 1,
  engine: '0.1.0-alpha.3',
  schema: 'definition user {}\n\ndefinition doc {\n    relation viewer: user\n}',
  relationships: '// 👩‍💻 astral and "quotes"\ndoc:a#viewer@user:anne',
  assertions: 'assertions:\n  assertTrue:\n    - doc:a#viewer@user:anne\n',
  query: { kind: 'lookup_subjects', text: 'doc:a#viewer@user' },
};

/** The payload for any JSON value, as a page would write it. */
async function pack(value: unknown): Promise<string> {
  return encodeState(value as State);
}

describe('share links', () => {
  it('round-trips the state', async () => {
    const payload = await encodeState(STATE);
    expect(payload).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(await decodeState(payload)).toEqual(STATE);
  });

  it('round-trips every corpus fixture, compressed', async () => {
    for (const c of corpus) {
      const s: State = { v: 1, engine: ENGINE, schema: c.schema, relationships: c.relationships, assertions: c.assertions, query: { kind: 'check', text: '' } };
      const payload = await encodeState(s);
      expect(await decodeState(payload)).toEqual(s);
      if (c.fixture === 'google_docs') expect(payload.length).toBeLessThan(4000);
    }
  });

  it('stamps the page engine version from the workspace', () => {
    expect(ENGINE).toMatch(/^\d+\.\d+\.\d+/);
  });

  const malformed: [string, () => Promise<string>][] = [
    ['not base64url', async () => 'abc+/def'],
    ['an impossible base64 length', async () => 'abcde'],
    ['not deflate-raw', async () => 'AAAAAAAA'],
    ['truncated', async () => (await encodeState(STATE)).slice(0, -6)],
    ['not JSON', async () => {
      const bytes = new TextEncoder().encode('{nope');
      const z = new Uint8Array(await new Response(new Blob([bytes]).stream().pipeThrough(new CompressionStream('deflate-raw'))).arrayBuffer());
      return btoa(String.fromCharCode(...z)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    }],
    ['an array', () => pack([])],
    ['another format version', () => pack({ ...STATE, v: 2 })],
    ['a missing part', () => pack({ v: 1, engine: STATE.engine, schema: '', relationships: '', query: STATE.query })],
    ['an extra part', () => pack({ ...STATE, extra: 1 })],
    ['a part that is not text', () => pack({ ...STATE, schema: 42 })],
    ['an unknown query kind', () => pack({ ...STATE, query: { kind: 'expand', text: '' } })],
    ['a query without text', () => pack({ ...STATE, query: { kind: 'check' } })],
    ['no engine version', () => pack({ ...STATE, engine: '' })],
  ];
  for (const [name, payload] of malformed) {
    it(`rejects a link that is ${name}, whole`, async () => {
      await expect(decodeState(await payload())).rejects.toBeInstanceOf(ShareError);
    });
  }

  it('refuses a state that inflates past its limit', async () => {
    const payload = await pack({ ...STATE, schema: ' '.repeat(5_000_000) });
    await expect(decodeState(payload)).rejects.toThrow(/larger than/);
  });
});

describe('version targets', () => {
  it('names a release only for a plain version', () => {
    expect(releaseOf('0.1.0-alpha.3')).toBe('0.1.0-alpha.3');
    expect(releaseOf('0.1.0')).toBe('0.1.0');
    expect(releaseOf('0.1.0-alpha.3+abc1234')).toBeNull();
    expect(releaseOf('../../evil')).toBeNull();
    expect(releaseOf('')).toBeNull();
  });

  it('finds the site root from any base', () => {
    expect(siteRoot('/pemba/')).toBe('/pemba/');
    expect(siteRoot('/pemba/v/0.1.0-alpha.3/')).toBe('/pemba/');
    expect(siteRoot('/pemba/next/')).toBe('/pemba/');
    expect(siteRoot('/')).toBe('/');
  });

  it('links the release page with the same fragment', () => {
    expect(releaseUrl('/pemba/next/', '0.1.0-alpha.2', '#s=abc')).toBe('/pemba/v/0.1.0-alpha.2/#s=abc');
  });
});
