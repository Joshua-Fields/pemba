// Release helpers for the Pages deploy (docs/11-playground.md §5.1):
//   node scripts/release.mjs notes <version> [CHANGELOG.md]   the version's CHANGELOG section
//   node scripts/release.mjs prerelease <version>              `true` if it has a prerelease part
//   node scripts/release.mjs newest <version>...               the release the site root serves
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/;

function parse(version) {
  const m = SEMVER.exec(version);
  if (!m) throw new Error(`not a version: ${version}`);
  return { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] ? m[4].split('.') : [] };
}

function compareIdentifiers(a, b) {
  const an = /^\d+$/.test(a);
  const bn = /^\d+$/.test(b);
  if (an && bn) return Number(a) - Number(b);
  // SemVer §11: numeric identifiers sort before alphanumeric ones.
  if (an !== bn) return an ? -1 : 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** SemVer 2.0.0 precedence: negative, zero or positive, like a sort comparator. */
export function compareVersions(a, b) {
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < 3; i++) {
    if (x.core[i] !== y.core[i]) return x.core[i] - y.core[i];
  }
  // A release outranks its prereleases.
  if (x.pre.length === 0 || y.pre.length === 0) return y.pre.length - x.pre.length;
  for (let i = 0; i < Math.min(x.pre.length, y.pre.length); i++) {
    const c = compareIdentifiers(x.pre[i], y.pre[i]);
    if (c !== 0) return c;
  }
  return x.pre.length - y.pre.length;
}

/**
 * Whether `version` has a prerelease part (`-alpha.N`, `-rc.N`), so its GitHub Release is
 * marked pre-release; a plain version is a full release (OQ-057).
 */
export function isPrerelease(version) {
  return parse(version).pre.length > 0;
}

/**
 * The release the site root serves: the newest stable release once one exists, and
 * until then the newest prerelease (§5.1). `undefined` when there are none.
 */
export function newest(versions) {
  const stable = versions.filter((v) => !isPrerelease(v));
  const pool = stable.length > 0 ? stable : versions;
  return [...pool].sort(compareVersions).at(-1);
}

/** The body of `## [<version>]` in a Keep a Changelog file, without its heading. */
export function changelogSection(changelog, version) {
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((l) => l.startsWith(`## [${version}]`));
  if (start === -1) throw new Error(`CHANGELOG.md has no section for ${version}`);
  const body = [];
  for (const line of lines.slice(start + 1)) {
    // The next section, or the link definitions that close the file.
    if (line.startsWith('## ') || /^\[[^\]]+\]:\s/.test(line)) break;
    body.push(line);
  }
  const text = body.join('\n').trim();
  if (text === '') throw new Error(`CHANGELOG.md section ${version} is empty`);
  return `${text}\n`;
}

function main(argv) {
  const [command, ...args] = argv;
  if (command === 'notes' && (args.length === 1 || args.length === 2)) {
    const file = args[1] ?? fileURLToPath(new URL('../../../CHANGELOG.md', import.meta.url));
    process.stdout.write(changelogSection(fs.readFileSync(file, 'utf8'), args[0]));
    return;
  }
  if (command === 'prerelease' && args.length === 1) {
    process.stdout.write(`${isPrerelease(args[0])}\n`);
    return;
  }
  if (command === 'newest') {
    const v = newest(args);
    if (v !== undefined) process.stdout.write(`${v}\n`);
    return;
  }
  console.error(
    'usage: node scripts/release.mjs notes <version> [CHANGELOG.md] | prerelease <version> | newest <version>...',
  );
  process.exit(2);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(e instanceof Error ? e.message : e);
    process.exit(1);
  }
}
