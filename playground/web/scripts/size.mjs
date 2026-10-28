// Fails when the built site is over its gzip budget (docs/11-playground.md §5):
//   node scripts/size.mjs <dist> <budget KB>
// Every file is counted gzipped at level 9, the way it travels.
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

const [dir, budget] = process.argv.slice(2);
if (!dir || !budget) {
  console.error('usage: node scripts/size.mjs <dist> <budget KB>');
  process.exit(2);
}
const files = fs.readdirSync(dir, { recursive: true, withFileTypes: true }).filter((e) => e.isFile());
let total = 0;
const rows = [];
for (const f of files) {
  const p = path.join(f.parentPath, f.name);
  const gz = zlib.gzipSync(fs.readFileSync(p), { level: 9 }).length;
  total += gz;
  rows.push([path.relative(dir, p), gz]);
}
rows.sort((a, b) => b[1] - a[1]);
for (const [name, gz] of rows) console.log(`${(gz / 1024).toFixed(1).padStart(8)} KB  ${name}`);
const kb = total / 1024;
console.log(`${kb.toFixed(1).padStart(8)} KB  total gzip (budget ${budget} KB)`);
if (kb > Number(budget)) {
  console.error(`over budget by ${(kb - Number(budget)).toFixed(1)} KB`);
  process.exit(1);
}
