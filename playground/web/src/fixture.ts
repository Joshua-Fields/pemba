// Fixture YAML export (docs/11-playground.md §4): `schema` and `relationships` as literal
// block scalars, then the assertions editor verbatim. Import is `parse_fixture` in the
// engine (`pemba-fixtures`), which splits such a file back into the same three texts.

/** The three editors. */
export interface Parts {
  schema: string;
  relationships: string;
  assertions: string;
}

/**
 * `key: |-` and `text` indented by two spaces. The header keeps `text` exact: the chomping
 * indicator matches its trailing newlines (`-` none, clip one, `+` more), and an explicit
 * indentation indicator is added when its first non-blank line starts with a space, which
 * YAML would otherwise take as the block's indentation.
 */
function block(key: string, text: string): { yaml: string; keep: boolean } {
  // Only line breaks: every one is kept, as empty lines.
  if (/^\n+$/.test(text)) return { yaml: `${key}: |+\n${text}`, keep: true };
  const lines = text.split('\n');
  let trailing = 0;
  while (trailing < lines.length - 1 && lines[lines.length - 1 - trailing] === '') trailing++;
  const chomp = trailing === 0 ? '-' : trailing === 1 ? '' : '+';
  const leading = /^[ \n]*/.exec(text)?.[0] ?? '';
  const indent = leading.includes(' ') ? '2' : '';
  const body = lines.slice(0, lines.length - trailing);
  // The first trailing newline ends the last line; each further one is an empty line.
  const tail = '\n'.repeat(Math.max(trailing - 1, 0));
  const content = text === '' ? '' : `${body.map((l) => (l === '' ? '' : `  ${l}`)).join('\n')}\n${tail}`;
  return { yaml: `${key}: |${indent}${chomp}\n${content}`, keep: chomp === '+' };
}

/** The fixture file for the three editors. */
export function exportFixture(parts: Parts): string {
  const lf = (s: string) => s.replace(/\r\n?/g, '\n');
  let out = '';
  for (const [key, text] of [['schema', parts.schema], ['relationships', parts.relationships]] as const) {
    const b = block(key, lf(text));
    // A blank line after a kept block would become part of its text, and one before the
    // assertions would become part of theirs (import gives column-0 blank lines to the
    // key after them).
    out += b.keep || key === 'relationships' ? b.yaml : `${b.yaml}\n`;
  }
  const assertions = lf(parts.assertions);
  if (assertions === '') return out;
  return out + (assertions.endsWith('\n') ? assertions : `${assertions}\n`);
}
