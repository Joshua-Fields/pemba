import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

import { afterEach, describe, expect, it } from 'vitest';

import { checkSite } from './check-site.mjs';
import { changelogSection, compareVersions, isPrerelease, newest } from './release.mjs';

describe('compareVersions', () => {
  it('follows SemVer precedence', () => {
    const sorted = [
      '0.1.0-alpha.1',
      '0.1.0-alpha.2',
      '0.1.0-alpha.10',
      '0.1.0-alpha.beta',
      '0.1.0-beta',
      '0.1.0-beta.2',
      '0.1.0-rc.1',
      '0.1.0',
      '0.1.1',
      '0.2.0',
      '1.0.0',
    ];
    expect([...sorted].reverse().sort(compareVersions)).toEqual(sorted);
  });

  it('rejects what is not a version', () => {
    expect(() => compareVersions('v0.1.0', '0.1.0')).toThrow(/not a version/);
  });
});

describe('isPrerelease', () => {
  it('is true only with a prerelease part', () => {
    expect(isPrerelease('0.1.0-alpha.4')).toBe(true);
    expect(isPrerelease('1.0.0-rc.1')).toBe(true);
    expect(isPrerelease('0.1.0')).toBe(false);
    expect(isPrerelease('1.2.3+build.5')).toBe(false);
  });
});

describe('newest', () => {
  it('is the newest prerelease until a stable release exists', () => {
    expect(newest(['0.1.0-alpha.9', '0.1.0-alpha.10', '0.1.0-alpha.2'])).toBe('0.1.0-alpha.10');
  });

  it('then is the newest stable release', () => {
    expect(newest(['0.1.0', '0.2.0-alpha.1', '0.1.1'])).toBe('0.1.1');
  });

  it('is undefined with no releases', () => {
    expect(newest([])).toBeUndefined();
  });
});

describe('changelogSection', () => {
  const changelog = [
    '# Changelog',
    '',
    '## [Unreleased]',
    '',
    '- next',
    '',
    '## [0.1.0-alpha.2] - 2026-10-27',
    '',
    '### Added',
    '- two',
    '',
    '## [0.1.0-alpha.1] - 2026-10-20',
    '',
    '- one',
    '',
    '[Unreleased]: https://example.com/a',
    '[0.1.0-alpha.1]: https://example.com/b',
  ].join('\n');

  it('is the body up to the next section', () => {
    expect(changelogSection(changelog, '0.1.0-alpha.2')).toBe('### Added\n- two\n');
  });

  it('stops at the link definitions after the last section', () => {
    expect(changelogSection(changelog, '0.1.0-alpha.1')).toBe('- one\n');
  });

  it('fails for a version without a section', () => {
    expect(() => changelogSection(changelog, '0.1.0-alpha.3')).toThrow(/no section/);
  });

  it('reads the real CHANGELOG.md', () => {
    const text = fs.readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8');
    expect(changelogSection(text, '0.1.0-alpha.4')).toMatch(/\S/);
  });
});

describe('checkSite', () => {
  let site = '';

  afterEach(() => {
    if (site !== '') fs.rmSync(site, { recursive: true, force: true });
  });

  function page(dir, base, version, label = dir === 'next') {
    fs.mkdirSync(path.join(site, dir), { recursive: true });
    fs.writeFileSync(
      path.join(site, dir, 'index.html'),
      `${label ? '<p class="dev-label">development build (main)</p>\n' : ''}` +
        `<script type="module" src="${base}assets/index.js"></script>\n<footer class="foot" data-version="${version}"></footer>\n`,
    );
  }

  function assemble() {
    site = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-site-'));
    page('v/0.1.0-alpha.4', '/pemba/v/0.1.0-alpha.4/', '0.1.0-alpha.4');
    page('v/0.1.0-alpha.5', '/pemba/v/0.1.0-alpha.5/', '0.1.0-alpha.5');
    page('', '/pemba/', '0.1.0-alpha.5');
    page('next', '/pemba/next/', '0.1.0-alpha.6');
  }

  const releases = ['0.1.0-alpha.4', '0.1.0-alpha.5'];

  it('accepts a complete site', () => {
    assemble();
    expect(checkSite(site, releases)).toEqual([]);
  });

  it('wants a /v/ for every release asset', () => {
    assemble();
    expect(checkSite(site, [...releases, '0.1.0-alpha.3'])).toEqual(['v/0.1.0-alpha.3: no index.html']);
  });

  it('wants the newest release at the root', () => {
    assemble();
    page('', '/pemba/', '0.1.0-alpha.4');
    expect(checkSite(site, releases)).toEqual(['/: footer version is 0.1.0-alpha.4, expected 0.1.0-alpha.5']);
  });

  it('wants each copy built for its own path', () => {
    assemble();
    page('v/0.1.0-alpha.4', '/pemba/', '0.1.0-alpha.4');
    expect(checkSite(site, releases)).toEqual(['v/0.1.0-alpha.4: /pemba/assets/index.js is outside /pemba/v/0.1.0-alpha.4/']);
  });

  it('wants /next/', () => {
    assemble();
    fs.rmSync(path.join(site, 'next'), { recursive: true });
    expect(checkSite(site, releases)).toEqual(['next: no index.html']);
  });

  it('wants /next/ labelled as a development build, and no release labelled so', () => {
    assemble();
    page('next', '/pemba/next/', '0.1.0-alpha.6', false);
    page('v/0.1.0-alpha.4', '/pemba/v/0.1.0-alpha.4/', '0.1.0-alpha.4', true);
    expect(checkSite(site, releases)).toEqual([
      'v/0.1.0-alpha.4: has a "development build (main)" label',
      'next: no "development build (main)" label',
    ]);
  });

  it('accepts a root that only points at /next/ before the first release', () => {
    site = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-site-'));
    fs.writeFileSync(path.join(site, 'index.html'), '<meta http-equiv="refresh" content="0; url=next/">');
    page('next', '/pemba/next/', '0.1.0-alpha.6');
    expect(checkSite(site, [])).toEqual([]);
  });
});
