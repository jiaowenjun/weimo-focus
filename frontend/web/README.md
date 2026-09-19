# Time Tracker Web

This package is the local React workbench for `weimo-time`. It talks to the
Rust backend over HTTP and never reads SQLite or calls Calendar directly.

## Development

Start the backend in one terminal, then run the Vite app from this package in
another terminal:

```bash
# Terminal 1: from the repo root
cargo run -p tracker-backend --locked -- --database ./data/tracker.sqlite3

# Terminal 2: from frontend/web/
pnpm install --frozen-lockfile
pnpm dev
```

The Vite server is available at `http://127.0.0.1:4300/` and proxies both
`/health` and `/api` to `http://127.0.0.1:9123` by default. For a temporary
backend on another port, set `TRACKER_BACKEND_URL` before starting Vite:

```bash
TRACKER_BACKEND_URL=http://127.0.0.1:9124 pnpm dev
```

## Checks

```bash
pnpm lint
pnpm exec tsc -b
pnpm test
pnpm build
```

The contract tests use fake `fetch` responses. They do not access a user's
SQLite database or Calendar.
