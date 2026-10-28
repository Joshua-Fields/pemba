// The request and answer shapes of the WASM API (docs/11-playground.md §3.1; the
// `pemba-playground` crate docs are the reference).

export type QueryKind = 'check' | 'lookup_resources' | 'lookup_subjects';

export interface RunRequest {
  schema: string;
  relationships: string;
  query: { kind: QueryKind; text: string; explain?: boolean };
}

/** What the page shows for one run. An error is never an answer. */
export type View =
  | { kind: 'check'; allowed: boolean; revision: number }
  | { kind: 'lookup_resources'; ids: string[]; revision: number }
  | { kind: 'lookup_subjects'; ids: string[]; wildcard: boolean; excluded: string[]; revision: number }
  | { kind: 'error'; code: string; message: string; stage: string };
