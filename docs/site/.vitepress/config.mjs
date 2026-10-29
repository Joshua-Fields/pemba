import fs from 'node:fs';
import { defineConfig } from 'vitepress';

const info = JSON.parse(fs.readFileSync(new URL('../generated/build-info.json', import.meta.url), 'utf8'));
const base = process.env.DOCS_BASE;
const items = info.complete ? [
  { text: 'Installation', link: '/guides/installation' },
  { text: 'Quick starts', link: '/quick-starts' },
  { text: 'Schema', link: '/guides/schema' },
  { text: 'Transactions', link: '/guides/transactions' },
  { text: 'Transactional tutorial', link: '/guides/transactional-tutorial' },
  { text: 'Consistency', link: '/guides/consistency' },
  { text: 'Limits and troubleshooting', link: '/guides/troubleshooting' },
  { text: 'API reference', link: '/api' },
  ...(info.contributing ? [{ text: 'Contributing', link: '/contributing' }] : []),
] : [];

export default defineConfig({
  title: 'pemba',
  description: 'Embedded authorisation: application data and permissions in one transaction.',
  lang: 'en-GB',
  base,
  srcDir: 'generated',
  ignoreDeadLinks: false,
  markdown: { config(md) { md.set({ html: false }); } },
  head: [['meta', { name: 'pemba-docs-channel', content: info.channel }],
    ['meta', { name: 'pemba-docs-version', content: info.version ?? 'none' }]],
  themeConfig: {
    docsLabel: info.label,
    nav: [
      { text: 'Documentation', link: '/' },
      { text: 'Playground', link: 'https://joshua-fields.github.io/pemba/' },
      { text: info.channel === 'next' ? 'Stable docs' : 'Development docs', link: info.channel === 'next' ? 'https://joshua-fields.github.io/pemba/docs/' : 'https://joshua-fields.github.io/pemba/docs/next/' },
    ],
    sidebar: items.length ? [{ text: info.label, items }] : [],
    outline: [2, 3],
    footer: { message: info.label, copyright: 'Apache-2.0 · pemba contributors' },
    search: info.complete ? { provider: 'local' } : undefined,
  },
});
