# Documentation site

The T7.1-approved VitePress site uses the existing Pages assembly. Guides remain in
`docs/guides/`; the build generates views, rewrites their local links and pins other
repository links to the selected source commit. Quick starts reuse README verbatim
(apart from presentation: image HTML is omitted and `zed` fences render as plain text).
Contributor content reuses root `CONTRIBUTING.md` when it exists in the selected
revision; older sources omit that navigation. Security and roadmap wait for T7.8–T7.9.

From the repository root, with Node 24:

```sh
npm ci --prefix docs/site
npm test --prefix docs/site
npm run build --prefix docs/site
npm run build --prefix docs/site -- --channel stable --empty true --out .vitepress/stable
npm ci --prefix playground/web
npx --prefix playground/web playwright install chromium
node docs/site/scripts/browser.mjs
```

The default build describes the current checkout at `/pemba/docs/next/`. Build warnings,
broken local links, anchors and missing assets fail the build. Output/cache/generated
Markdown are ignored. Source snippets are exercised by the existing Rust/Python tests;
the site does not maintain a second copy. The Chromium harness uses a static HTTP server
with real 404s and checks wide/narrow navigation, keyboard access, labels and assets.

`scripts/assemble.mjs <site> <release-assets>` runs after playground assembly. Stable
docs use the newest stable tag represented by the release assets, development uses
main, and stable is built first so its output cannot erase the nested development
tree. The builder and lockfile always come from main, including on an older-tag
dispatch. No stable release means a labelled placeholder, not development content
presented as stable. A legacy stable tag lacking site source links its exact README
and release notes (OQ-063). No new whole-site deployment or historical docs archive.

Dependencies (npm registry checked 2026-10-29):

- VitePress **1.6.4**, current stable generator: approved by OQ-059; provides static
  rendering, responsive/keyboard navigation and local search.
- Vue **3.5.44**: direct theme dependency for a visible release/development banner
  on every guide, including narrow screens; already required by VitePress.
- Vite **6.4.5** override under VitePress: the patched 6.x line removes the
  unpatched dev-server/editor advisories in VitePress's default Vite 5/esbuild tree.
  Its Vite Vue plugin supports Vite 6; both builds and browser navigation are verified.
  See [upstream advisory](https://github.com/advisories/GHSA-587h-7wmj-h5pm).

The override is scoped to this package; the playground's dependencies are unchanged.
Full docs-tooling `npm audit` reports zero vulnerabilities. Playwright reuses the
existing playground dependency rather than introducing a second browser toolchain.
