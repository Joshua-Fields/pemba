import { defineConfig } from 'vitest/config';

// `PLAYGROUND_BASE` is the path the site is served under (docs/11-playground.md §5.1):
// `/pemba/`, `/pemba/v/<version>/`, `/pemba/next/`. Locally, the root.
export default defineConfig({
  base: process.env['PLAYGROUND_BASE'] ?? '/',
  build: { target: 'es2022' },
  worker: { format: 'es' },
  test: { include: ['src/**/*.test.ts'] },
});
