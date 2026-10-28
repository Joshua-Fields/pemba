// The example menu (docs/11-playground.md §4). Examples are `testdata/fixtures` files, so
// `cargo test` keeps them correct; the page loads them as it imports any fixture.

import agentAuthz from '../../../testdata/fixtures/agent_authz.yaml?raw';
import githubLike from '../../../testdata/fixtures/github_like.yaml?raw';
import googleDocs from '../../../testdata/fixtures/google_docs.yaml?raw';
import type { QueryKind } from './types';

/** One menu entry: the fixture text and a query that shows the model off. */
export interface Example {
  name: string;
  label: string;
  source: string;
  query: { kind: QueryKind; text: string };
}

export const EXAMPLES: readonly Example[] = [
  {
    name: 'google_docs',
    label: 'Google Docs: folders, groups, sharing',
    source: googleDocs,
    query: { kind: 'check', text: 'document:roadmap#view@user:bob' },
  },
  {
    name: 'github_like',
    label: 'GitHub: orgs, nested teams, repository roles',
    source: githubLike,
    query: { kind: 'check', text: 'repository:api#admin@user:sam' },
  },
  {
    name: 'agent_authz',
    label: "AI agents: tool calls on a person's behalf",
    source: agentAuthz,
    query: { kind: 'lookup_resources', text: 'tool#invoke@agent:triage' },
  },
];
