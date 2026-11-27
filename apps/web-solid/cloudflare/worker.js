// Only /api/* reaches this script; every other path is a static asset or the SPA fallback.
// No engine runs behind this host, so the app sees the same answer a paired-only listener gives.
export default {
  fetch() {
    return new Response(null, { status: 404, headers: { "cache-control": "no-store" } });
  },
};
