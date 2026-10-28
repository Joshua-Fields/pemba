import { StringStream } from '@codemirror/language';
import { describe as group, expect, it } from 'vitest';

import { dslParser } from './dsl';

/** `[text, style]` for every styled token of `source`, as the editor would colour it. */
function tokens(source: string): [string, string][] {
  const state = dslParser.startState?.(2);
  if (!state) throw new Error('no start state');
  const out: [string, string][] = [];
  for (const line of source.split('\n')) {
    const stream = new StringStream(line, 4, 2);
    while (!stream.eol()) {
      const style = dslParser.token(stream, state);
      if (stream.pos === stream.start) throw new Error(`no progress at ${stream.pos} in ${line}`);
      if (style) out.push([stream.current(), style]);
      stream.start = stream.pos;
    }
  }
  return out;
}

group('schema highlighting', () => {
  it('colours a definition', () => {
    expect(
      tokens(`definition docs/document {
    relation viewer: user | group#member | user:*
    permission view = viewer + parent->view
}`),
    ).toEqual([
      ['definition', 'keyword'],
      ['docs/document', 'typeName.definition'],
      ['{', 'bracket'],
      ['relation', 'keyword'],
      ['viewer', 'propertyName.definition'],
      [':', 'punctuation'],
      ['user', 'typeName'],
      ['|', 'punctuation'],
      ['group', 'typeName'],
      ['#', 'punctuation'],
      ['member', 'propertyName'],
      ['|', 'punctuation'],
      ['user', 'typeName'],
      [':', 'punctuation'],
      ['*', 'atom'],
      ['permission', 'keyword'],
      ['view', 'propertyName.definition'],
      ['=', 'punctuation'],
      ['viewer', 'variableName'],
      ['+', 'operator'],
      ['parent', 'variableName'],
      ['->', 'operator'],
      ['view', 'variableName'],
      ['}', 'bracket'],
    ]);
  });

  it('keeps doc and block comments across lines', () => {
    expect(tokens('/** the\n doc */ /* x\n y */ // z\n/**/')).toEqual([
      ['/** the', 'docComment'],
      [' doc */', 'docComment'],
      ['/* x', 'comment'],
      [' y */', 'comment'],
      ['// z', 'comment'],
      ['/**/', 'comment'],
    ]);
  });

  it('reads unsupported SpiceDB words as keywords and marks stray characters', () => {
    expect(tokens('use typechecking\ncaveat c ?')).toEqual([
      ['use', 'keyword'],
      ['typechecking', 'variableName'],
      ['caveat', 'keyword'],
      ['c', 'typeName.definition'],
      ['?', 'invalid'],
    ]);
  });
});
