// Highlighting for the schema DSL (docs/03-schema-language.md §2). It colours tokens only:
// what the schema means, and every error in it, comes from the engine's `compile`.

import { StreamLanguage, type StreamParser } from '@codemirror/language';

interface State {
  /** Inside a block comment that started on an earlier line; `doc` for `/** … *\/`. */
  comment: 'plain' | 'doc' | null;
  /** The name the last keyword introduces. */
  naming: 'type' | 'member' | null;
  /** Inside a relation's allowed types (after its `:`), where names are types. */
  types: boolean;
  /** The last token was `#`, so a name here is a relation. */
  hash: boolean;
}

// `caveat`, `use`, `with`, `self` and `nil` are SpiceDB's; the parser rejects them with
// `E_UNSUPPORTED_FEATURE` (03 §7), but they still read as keywords.
const KEYWORDS = new Set(['definition', 'relation', 'permission', 'caveat', 'use', 'with', 'self', 'nil']);

function blockComment(stream: Parameters<StreamParser<State>['token']>[0], state: State): string {
  const style = state.comment === 'doc' ? 'docComment' : 'comment';
  if (stream.skipTo('*/')) {
    stream.match('*/');
    state.comment = null;
  } else {
    stream.skipToEnd();
  }
  return style;
}

export const dslParser: StreamParser<State> = {
  name: 'pemba-schema',
  startState: () => ({ comment: null, naming: null, types: false, hash: false }),
  copyState: (s) => ({ ...s }),
  token(stream, state) {
    if (state.comment) return blockComment(stream, state);
    if (stream.eatSpace()) return null;
    if (stream.match('//')) {
      stream.skipToEnd();
      return 'comment';
    }
    if (stream.match('/**/')) return 'comment';
    if (stream.match('/**')) {
      state.comment = 'doc';
      return blockComment(stream, state);
    }
    if (stream.match('/*')) {
      state.comment = 'plain';
      return blockComment(stream, state);
    }
    const hash = state.hash;
    state.hash = false;
    const word = stream.match(/^[a-z_][a-z0-9_]*/) as RegExpMatchArray | null;
    if (word) {
      const text = word[0];
      if (KEYWORDS.has(text)) {
        state.naming = text === 'definition' || text === 'caveat' ? 'type' : text === 'use' ? null : 'member';
        state.types = false;
        return 'keyword';
      }
      const naming = state.naming;
      state.naming = null;
      // `docs/document`: the prefix and `/` belong to the type name.
      const prefixed = stream.match(/^\/[a-z_][a-z0-9_]*/) !== null;
      if (naming === 'type') return 'typeName.definition';
      if (naming === 'member') {
        // Only `relation name:` is followed by types; a permission's `=` is not `:`.
        state.types = stream.match(/^\s*:/, false) !== null;
        return 'propertyName.definition';
      }
      if (hash) return 'propertyName';
      return state.types || prefixed ? 'typeName' : 'variableName';
    }
    state.naming = null;
    if (stream.match('->')) return 'operator';
    const ch = stream.next() ?? '';
    if (ch === '#') state.hash = true;
    if (ch === '}') state.types = false;
    if ('+&-'.includes(ch)) return 'operator';
    if (ch === '*') return 'atom';
    if ('{}()'.includes(ch)) return 'bracket';
    if (':=|#,./'.includes(ch)) return 'punctuation';
    // `E_UNEXPECTED_CHAR`; the engine's diagnostic says so.
    return 'invalid';
  },
  languageData: {
    commentTokens: { line: '//', block: { open: '/*', close: '*/' } },
  },
};

/** The schema editor's language. */
export const dsl = StreamLanguage.define(dslParser);
