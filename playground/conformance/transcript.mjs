// WASM side of the playground conformance check (docs/11-playground.md §6, T6.3): the
// transcript of every manifest entry, from the release `.wasm` through wasm-bindgen's
// `--target nodejs` glue. The native side writes the same file with the same Rust
// function (`pemba_playground::transcript`); `run.sh` diffs the two.
//
//   node transcript.mjs <pkg-dir> <manifest> <out>
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';

const [pkg, manifest, out] = process.argv.slice(2);
if (!out) {
  console.error('usage: node transcript.mjs <pkg-dir> <manifest> <out>');
  process.exit(2);
}
const { transcript } = createRequire(import.meta.url)(path.resolve(pkg, 'pemba_playground.js'));
const names = fs.readFileSync(manifest, 'utf8').split('\n').filter((l) => l !== '');
const fd = fs.openSync(out, 'w');
let lines = 0;
for (const name of names) {
  // Read as UTF-8 text, as the native side does; the bytes reach Rust unchanged.
  const text = transcript(name, fs.readFileSync(name, 'utf8'));
  lines += text.split('\n').length - 1;
  fs.writeSync(fd, text);
}
fs.closeSync(fd);
console.error(`${lines} lines in ${out}`);
