// Editor colours from the page's CSS variables (style.css), so the editors follow the
// light and dark themes; in dark mode, One Dark's syntax colours replace the defaults,
// which are unreadable on a dark background.

import { syntaxHighlighting } from '@codemirror/language';
import { Compartment, type Extension } from '@codemirror/state';
import { oneDarkHighlightStyle } from '@codemirror/theme-one-dark';
import { EditorView } from 'codemirror';

const base = EditorView.theme({
  '&': { color: 'var(--text)', backgroundColor: 'var(--panel)' },
  '.cm-gutters': {
    backgroundColor: 'var(--panel)',
    color: 'var(--muted)',
    borderRight: '1px solid var(--border)',
  },
  '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'var(--active-line)' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--text)' },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground':
    { backgroundColor: 'var(--selection)' },
});

const scheme = new Compartment();
const dark = window.matchMedia('(prefers-color-scheme: dark)');
const views: EditorView[] = [];

function current(): Extension {
  return dark.matches ? [EditorView.darkTheme.of(true), syntaxHighlighting(oneDarkHighlightStyle)] : [];
}

/** The theme extension for a new editor. */
export function theme(): Extension {
  return [base, scheme.of(current())];
}

/** Keeps `view` in step with the system colour scheme. */
export function follow(view: EditorView): void {
  views.push(view);
}

dark.addEventListener('change', () => {
  for (const v of views) v.dispatch({ effects: scheme.reconfigure(current()) });
});
