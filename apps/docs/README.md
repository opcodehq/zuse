# Zuse documentation

The documentation site (docs.zuse.sh) is a Next.js and Fumadocs app,
exported as static files and served by Cloudflare Workers.

## Content boundary

Only `content/docs` is published, through the collection in `source.config.ts`.
Search, sitemap, and LLM exports all use that same collection.
Repository operator runbooks belong in `internal-docs/`, not this collection.
Do not copy or import internal runbooks into public pages. This README is for
docs-site maintainers and is not part of the published collection.

## Local development

The docs app installs on its own, like `apps/website`. It shares the
website's `@zuse/analytics` package through a `file:` dependency.

```bash
cd apps/docs
bun install --frozen-lockfile
bun run dev
```

The local site is available at `http://localhost:3002`.

## Navigation and pages

- Sections (the sidebar switcher) are assembled in `src/lib/source.ts`. Each
  content folder becomes a labeled group; its `meta.json` `title` is the group
  label and `pages` sets the order.
- Every page sets an `icon` (a Lucide name, or `logo:<name>` for a provider mark
  in `public/logos`) and may set a short `navTitle` for the sidebar.
- Screenshots live in `public/screenshots` as WebP. Use `<Screenshot>` for a
  framed image on brand artwork and `<FeatureGrid>`/`<Feature>` for linked
  image cards.

## Hosting

The site is a static export (`output: "export"`) served from Cloudflare Workers
static assets as the `zuse-docs` Worker (`wrangler.jsonc`). `bun run build`
writes the site to `out/`, including search (`/api/search`), `llms.txt`,
`llms-full.txt`, `sitemap.xml`, `og.png`, and one Markdown file per page under
`out/api/markdown`.

`worker/index.ts` is the only code that runs per request. It runs before the
assets for page routes and:

- redirects `/` to `/start` and `/providers/gemini` to the ACP page,
- serves `/<page>.md` (and the older `/api/markdown/<page>`) from the exported
  Markdown,
- serves that Markdown instead of HTML when the request prefers
  `text/markdown` or `text/plain`, or comes from a known AI crawler.

Static files such as `/_next/*`, images, `llms.txt`, and `openapi.json` skip the
Worker; their headers come from `public/_headers`.

Preview the production Worker locally with `bun run preview`
(`http://localhost:3002`). Production deploys through Cloudflare Workers Builds:

| Setting | Value |
| --- | --- |
| Root directory | `apps/docs` |
| Build command | `bun run build` |
| Deploy command | `bunx wrangler deploy` |
| Build variables | `BUN_VERSION=1.4.4`, `NEXT_PUBLIC_POSTHOG_KEY`, `NEXT_PUBLIC_POSTHOG_HOST` (optional) |

The PostHog variables are read at build time, so set them as build variables,
not Worker runtime variables. The `Docs` GitHub workflow only builds and checks
the site.
