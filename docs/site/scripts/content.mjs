import fs from 'node:fs';
import path from 'node:path';
import { isPrerelease, newest } from '../../../playground/web/scripts/release.mjs';

const guides = ['installation', 'schema', 'transactions', 'transactional-tutorial', 'consistency', 'troubleshooting'];
const pages = new Map([
  ['README.md', 'quick-starts'],
  ...guides.map(name => [`docs/guides/${name}.md`, `guides/${name}`]),
]);

/** Reuse the release classifier; a newer alpha can never replace stable docs. */
export function stableVersion(versions) {
  return newest(versions.filter(version => !isPrerelease(version)));
}

/** Generate views of the selected checkout, never copies maintained separately. */
export function prepare(source, out, info) {
  fs.mkdirSync(out, { recursive: true });
  const available = fs.existsSync(path.join(source, 'docs/site/package.json'));
  const published = info.channel === 'stable';
  const repo = `https://github.com/Joshua-Fields/pemba/blob/${info.commit}`;
  const label = published
    ? info.version ? `Stable release ${info.version}` : 'No stable release is available'
    : `development build (main) — ${info.version} — ${info.commit.slice(0, 7)}`;
  const complete = available && (!published || !!info.version);
  const contributing = complete && fs.existsSync(path.join(source, 'CONTRIBUTING.md'));
  const security = complete && fs.existsSync(path.join(source, 'SECURITY.md'));
  const sourcePages = new Map(pages);
  if (contributing) sourcePages.set('CONTRIBUTING.md', 'contributing');
  if (security) sourcePages.set('SECURITY.md', 'security');
  const metadata = { ...info, label, complete, contributing, security };
  fs.writeFileSync(path.join(out, 'build-info.json'), JSON.stringify(metadata));
  const next = '[Development documentation](https://joshua-fields.github.io/pemba/docs/next/)';
  if (!complete) {
    const message = info.version
      ? `This release predates the documentation site. Read its [README](${repo}/README.md) and [release notes](https://github.com/Joshua-Fields/pemba/releases/tag/v${info.version}).`
      : 'No stable release has been published. Current guides describe development APIs and must not be treated as stable documentation.';
    fs.writeFileSync(path.join(out, 'index.md'), `# ${label}\n\n${message}\n\n${next}\n`);
    return metadata;
  }
  for (const [file, route] of sourcePages) {
    let text = fs.readFileSync(path.join(source, file), 'utf8');
    text = text.replace(/^```zed$/gm, '```text');
    if (file === 'README.md') text = text.replace(/<p\b[^>]*>[\s\S]*?<\/p>/g, '');
    // Resolve Markdown links from the source file, preserving anchors and routing
    // copied guides locally. Other repository material stays pinned to this commit.
    text = text.replace(/\]\(([^\s)]+)\)/g, (all, href) => {
      const own = /^https:\/\/github\.com\/Joshua-Fields\/pemba\/(?:blob|tree)\/main\/(.+)$/.exec(href);
      if (!own && /^(?:[a-z]+:|\/|#)/i.test(href)) return all;
      const [target, anchor] = (own?.[1] ?? href).split('#');
      const resolved = own ? target : path.posix.normalize(path.posix.join(path.posix.dirname(file), target));
      if (!fs.existsSync(path.join(source, resolved))) throw new Error(`${file}: missing ${href}`);
      const local = sourcePages.get(resolved);
      const sourceUrl = fs.statSync(path.join(source, resolved)).isDirectory() ? repo.replace('/blob/', '/tree/') : repo;
      const url = local ? `/${local}` : `${sourceUrl}/${resolved}`;
      return `](${url}${anchor ? `#${anchor}` : ''})`;
    });
    const dest = path.join(out, `${route}.md`);
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.writeFileSync(dest, text);
  }
  fs.writeFileSync(path.join(out, 'index.md'), `# pemba documentation\n\n${label}\n\npemba keeps application data and permissions in one database transaction.\n\nStart with [installation and quick starts](guides/installation.md), then run the [transactional-write tutorial](guides/transactional-tutorial.md).\n\nUse the [memory-store playground](https://joshua-fields.github.io/pemba/) to explore a schema; real database commit and rollback need the tutorial.\n`);
  fs.writeFileSync(path.join(out, 'api.md'), `# API reference\n\nThese pages describe ${label}.\n\n- [Rust API on docs.rs](https://docs.rs/pemba/${info.version}/pemba/) describes the published version; development additions may not be published yet.\n- [Current Rust and Python API specification](${repo}/docs/06-api.md).\n- Python includes runtime docstrings, type stubs and \`py.typed\`: use \`help(pemba.Engine)\` on the installed extension.\n- [Migration notes](${repo}/CHANGELOG.md).\n`);
  return metadata;
}
