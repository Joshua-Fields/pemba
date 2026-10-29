import fs from 'node:fs';
import path from 'node:path';

export function files(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const file = path.join(dir, entry.name);
    return entry.isDirectory() ? files(file) : [file];
  });
}

/** Validate generated local pages, anchors and assets against static hosting. */
export function linkProblems(out, base) {
  const problems = [];
  for (const file of files(out).filter(name => name.endsWith('.html'))) {
    const html = fs.readFileSync(file, 'utf8');
    const relative = path.relative(out, file).replaceAll(path.sep, '/');
    for (const [, href] of html.matchAll(/\b(?:href|src)="([^"]+)"/g)) {
      if (/^(?:[a-z]+:|\/\/)/i.test(href)) continue;
      const url = new URL(href.replaceAll('&amp;', '&'), `https://local.invalid${base}${relative}`);
      // The playground and opposite docs channel are assembled separately.
      if (!url.pathname.startsWith(base)) {
        if (['/pemba/', '/pemba/docs/', '/pemba/docs/next/'].includes(url.pathname)) continue;
        problems.push(`${relative}: outside base: ${href}`);
        continue;
      }
      let target = path.join(out, decodeURIComponent(url.pathname.slice(base.length)));
      if (url.pathname.endsWith('/')) target = path.join(target, 'index.html');
      if (!fs.existsSync(target) && !path.extname(target)) target += '.html';
      if (!fs.existsSync(target) || !fs.statSync(target).isFile()) {
        problems.push(`${relative}: missing ${href}`);
      } else if (url.hash && target.endsWith('.html')) {
        const ids = [...fs.readFileSync(target, 'utf8').matchAll(/\bid="([^"]+)"/g)].map(m => m[1]);
        if (!ids.includes(decodeURIComponent(url.hash.slice(1)))) problems.push(`${relative}: missing anchor ${href}`);
      }
    }
  }
  return problems;
}
