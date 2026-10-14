# Frameworks

What `scripts/preview.mjs` detects and how it runs each one. The script's `FRAMEWORKS` table
mirrors this file; change both together.

Detection order for a project: the project's own way to run everything first (root `dev` script
that is a shell script or runs turbo/nx/concurrently/npm-run-all, then `dev.sh`, `scripts/dev.sh`,
`bin/dev`, `Procfile.dev`, a `dev:` target in a `Makefile`), then one app (the workspace app the
current directory is in, the one with recent changes, or the only web app), then Python, Rails,
`docker compose`, and static files.

Package manager: `packageManager` in `package.json`, else the lockfile (`bun.lock`/`bun.lockb`,
`pnpm-lock.yaml`, `yarn.lock`, `package-lock.json`), else npm. Script arguments: npm needs
`npm run dev -- <args>`; pnpm, yarn and bun take `<pm> run dev <args>` (a `--` would reach the
script literally).

Readiness: Hako treats any HTTP answer at `http://127.0.0.1:<port><ready_path>` (or `[::1]`) as
ready, within 180 s; `ready_path` is `/` for all of these.

| Framework | Recognised by | Dev command | Port flag | Default port | Quirks |
|---|---|---|---|---|---|
| Next.js | dep `next`, `next.config.*` | `<pm> run dev` | `-p N` | 3000 | Next 15+ silently moves to the next port when the port is busy, so the port is checked first. Next 16 refuses a second `next dev` in the same app ("Unable to acquire lock"). First request compiles the page, so the first answer can take a while. |
| Vite (React, Vue, Svelte, Solid, Preact) | dep `vite`, `vite.config.*` | `<pm> run dev` | `--port N --strictPort` | 5173 | Without `--strictPort` Vite moves to the next free port and Hako waits on the wrong one. Binds `localhost`, which can be IPv6 only (`::1`); Hako checks both. `server.port` in the config is read. |
| SvelteKit | dep `@sveltejs/kit` | `<pm> run dev` | `--port N --strictPort` | 5173 | Vite underneath. |
| Astro | dep `astro`, `astro.config.*` | `<pm> run dev` | `--port N` | 4321 | Moves to the next port when busy (port checked first). |
| Nuxt | dep `nuxt` | `<pm> run dev` | `--port N` | 3000 | |
| React Router 7 / Remix (Vite) | dep `@react-router/dev` / `@remix-run/dev` with `vite.config.*` | `<pm> run dev` | `--port N` | 5173 | Classic Remix (no Vite config) defaults to 3000. |
| Angular | dep `@angular/cli`, `angular.json` | `<pm> run start` or `dev` | `--port N` | 4200 | `angular.json` `serve.options.port` is read. |
| Gatsby | dep `gatsby` | `<pm> run develop` | `-p N` | 8000 | |
| Create React App | dep `react-scripts` | `<pm> run start` | env `PORT=N` | 3000 | Set `BROWSER=none`; it asks interactively when the port is busy. |
| Eleventy | dep `@11ty/eleventy` | `<pm> run dev`/`start` | `--port=N` | 8080 | |
| Node server (Hono, Express, Fastify, Koa, ws) | those deps | `<pm> run dev` | env `PORT=N` | from code (`PORT || n`, `.listen(n)`) else 3000 | Only works when the code reads `PORT`; otherwise the port in the code is fixed. |
| NestJS | dep `@nestjs/core` | `<pm> run start:dev`/`dev` | env `PORT=N` | 3000 | |
| Django | `manage.py` | `.venv/bin/python manage.py runserver` | `127.0.0.1:N` | 8000 | Virtualenv `.venv`, `venv` or `env`. |
| Flask | `flask` in requirements/pyproject | `.venv/bin/flask --app <file> run --debug` | `--port N` | 5000 | On macOS port 5000 (and 7000) is usually taken by AirPlay Receiver (ControlCenter), so the next free port is used. Python buffers output into a pipe: `PYTHONUNBUFFERED=1` is set so the log is live. |
| FastAPI | `fastapi` in requirements/pyproject | `.venv/bin/uvicorn <module>:app --reload` | `--port N` | 8000 | |
| Rails | `rails` in `Gemfile` | `bin/dev` (Procfile.dev) or `bin/rails server -b 127.0.0.1` | env `PORT=N` / `-p N` | 3000 | `bundle check` decides whether to install. |
| docker compose | `docker-compose.yml`/`compose.yaml` | `docker compose up` | (fixed by the file) | first published port of the web service | Needs Docker running; the script starts Docker Desktop when a profile needs it. |
| Static HTML | `index.html` in `.`, `public`, `site`, `docs`, `dist` or `build` | `node preview.mjs serve` | `--port N` | 8080 | Small built-in server, no caching. |

## Ports the browser uses (multi-app projects)

When one script starts several apps, the preview registers every port the browser may call: the
web app (main port), API and WebSocket servers, and object storage or mail catchers from the
compose file (MinIO, LocalStack, Azurite: `storage`; Mailpit, MailHog, MailDev: `mail`).
Databases, caches and workers are not registered. Ports come from the dev script (a line naming
one app and a `localhost:N`, `--port N` or `PORT=N`), the apps' own scripts and config files,
`.env*` files (read only), and the compose file's published ports. A second web app started by the
same script (for example an admin app) becomes its own preview, registered with `attach` once it
answers, together with its partner API (`admin-api` for `admin`).
