This is a [Next.js](https://nextjs.org) project bootstrapped with [`create-next-app`](https://nextjs.org/docs/app/api-reference/cli/create-next-app).

## Getting Started

First, run the development server:

```bash
npm install
npm run dev
```

Open [http://localhost:3000](http://localhost:3000) with your browser to see the result.

You can start editing the page by modifying `app/page.tsx`. The page auto-updates as you edit the file.

Fontsource packages bundle the site fonts locally, so production builds do not fetch fonts from Google Fonts. Font family variables are defined in `components/site-typography.css`.

## Learn More

To learn more about Next.js, take a look at the following resources:

- [Next.js Documentation](https://nextjs.org/docs) - learn about Next.js features and API.
- [Learn Next.js](https://nextjs.org/learn) - an interactive Next.js tutorial.

You can check out [the Next.js GitHub repository](https://github.com/vercel/next.js) - your feedback and contributions are welcome!

## Deploy on Cloudflare Workers

The site runs on Cloudflare Workers through the [OpenNext Cloudflare adapter](https://opennext.js.org/cloudflare). `wrangler.jsonc` declares the Worker and its bindings, and `open-next.config.ts` keeps ISR pages in an R2 bucket revalidated by a Durable Object queue.

- `bun run cf:preview` builds the Worker and serves it locally with Wrangler on http://localhost:8787.
- `bun run cf:deploy` builds and deploys it by hand (needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`).

Production deploys come from Cloudflare Workers Builds, connected to this repository with root `apps/website`: every push to `main` runs `bun install && bun run cf:build`, then `npx opennextjs-cloudflare deploy`. `.github/workflows/website.yml` only checks and builds the site on pull requests and on `main`.
