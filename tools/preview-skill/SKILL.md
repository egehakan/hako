---
name: preview
description: Starts the project you changed (web app, website, or frontend with its backends and Docker services) as a Hako preview, so the owner can open it on their phone under Previews (a real Chrome on the Mac) and use it as they would on the Mac. It works out which repo and app, the framework, package manager, ports and services, remembers this per project so later runs are fast, and fixes and records start-up problems.
when_to_use: When the user wants to see, try, test or check what was built or changed, for example "preview this", "let me see it", "show me the app on my phone", "test the website", "run the app so I can check it", "start the dev server", "open it on my phone", "/preview". Also offer it after finishing a UI change.
argument-hint: "[directory or app name]"
allowed-tools: Bash(node ~/.claude/skills/preview/scripts/preview.mjs:*), Bash(node ~/.claude/skills/preview/scripts/preview.mjs *), Read
---

# Preview

The owner runs Claude Code inside **Hako** (their build of the Warp terminal). Hako runs dev servers
in visible tabs titled `Preview: <name>`. The phone app **Hako Remote** lists them under
**Previews** and opens them in a real Chrome on the Mac by default, so `localhost` URLs, cookies,
backends, object storage links and "Sign in with Google" work exactly as on the Mac. Your job is to
start **every process the browser needs** and register **all their ports**, nothing more: do not
rewrite URLs or configuration for the phone.

All the mechanics are in one script. Always call it exactly like this (Bash tool, `timeout: 600000`),
as the whole command: no `cd … &&` in front (pass `--dir` instead), no pipes or redirections, so
it matches the skill's pre-approved command:

```
node ~/.claude/skills/preview/scripts/preview.mjs <command> [options]
```

Do not start dev servers yourself with other commands (`npm run dev` and the like) while inside
Hako: they would not appear on the phone. If the script cannot be run, tell the owner why.

## Steps

1. **Read `~/.claude/skills/preview/LEARNINGS.md`** (short; general lessons from earlier runs).
2. **Choose the directory.** If you changed files in this session, use the repo (or app folder)
   you changed: `--dir <path>`. Otherwise the current directory. In a folder of repos the script
   picks the one with uncommitted changes or commits in the last hour, and asks when unsure.
   If the user named an app (`/preview admin`), pass `--app admin`.
3. **Start:** `start --dir <path>`. With a saved profile this is the fast path: it checks the
   profile still fits, reuses a preview that is already running, or starts it and waits.
   Without one it detects, prints the plan, starts, and saves the profile on success.
4. **Act on the exit code** (the output says what to do):

| Exit | Output starts with | What you do |
|---|---|---|
| 0 | `READY:` | Report (see "Report"). |
| 2 | `ASK:` | Ask the owner ONE short question, list the options and say which one you recommend (the script marks it). Rerun with their choice (`--dir`, `--app`, `--port`), or `--yes` for the recommendation. |
| 2 | `PORTS_BUSY:` | If the holders belong to this project (the script says so), rerun with `--free-ports` without asking and mention it. If another program holds the port, ask the owner before anything is stopped. |
| 2 | `NEEDS_INSTALL:` | Tell the owner what will be installed (project-local only), then rerun with `--install`. |
| 3 | `NOT_IN_HAKO:` | Say this terminal is not a Hako terminal, so nothing can appear on the phone. Offer to run the printed command here instead (Bash with `run_in_background: true`) and give the `http://localhost:<port>` address. |
| 4 | `STILL_STARTING:` | Run `wait` (same `--dir`), up to two more times, showing progress. |
| 1 | `FAILED:` or another message | Go to "When it fails". |

5. **Report** (see below).

## When it fails

The script prints Hako's message, the last lines of the server's output (every command started by
this skill also writes to `~/.hako/previews/logs/<id>.log`; read more with `logs --lines 200`) and
`DIAGNOSIS:`/`TRY:` lines for common problems. Then:

1. Find the cause in the log. Read the project's README, `package.json` scripts, config files and
   the dev script when the log is not enough.
2. Fix it with the least invasive option:
   - dependencies missing or out of date: `start --install`;
   - Docker not running: `start` starts Docker Desktop itself for projects that need it; if a
     service is missing, `docker compose up -d <service>` in the project;
   - a leftover server of this project holds a port: `start --free-ports` (or `free-port <port>`);
   - the port is taken by something else: `start --port <n>` (single apps) or ask the owner;
   - wrong script, directory, flags or environment: change the profile, for example
     `profile set previews.0.command="pnpm run dev:web"`, `profile set previews.0.directory=/abs/path`,
     `profile set previews.0.env.NODE_OPTIONS=--max-old-space-size=4096`,
     `profile set previews.0.ports='[{"port":8087,"label":"api"}]'`
     (values are JSON when they parse as JSON, else strings; `key=` deletes a key);
   - an app the script cannot work out: read the README, then
     `start --command "<command>" --port <n> [--label web]` (saved in the profile).
3. Retry. Stop after three failed attempts and tell the owner what you tried and what the log says.
4. **Record what you learned** once it works:
   - project-specific facts: `profile note "<what was wrong and what fixed it>"`;
   - a general lesson (a framework quirk, a tool flag, a macOS trap) that would help in other
     projects: `learn --situation "<one line>" --fix "<one line>"` (no local paths or project names;
     the script de-duplicates);
   - a bug or gap in the script itself: fix `~/.claude/skills/preview/scripts/preview.mjs` (keep it
     dependency-free) and add a learning.

## Rules

- Never edit the project's committed files (code, config, `.env*`) without asking the owner first.
  Propose the exact change. Installing dependencies into the project (`node_modules`, `.venv`) is
  fine; global installs (`npm -g`, `brew`) are not.
- Prefer the project's own way to run everything (a root `dev` script, `dev.sh`, `Procfile.dev`,
  `make dev`, `docker compose`), which the script does. One preview then carries every port the
  browser uses (web, API, WebSocket, object storage); a second web app started by the same script
  (an admin app) becomes a second preview that is registered (`attach`) once it answers.
- Only processes that belong to this project may be stopped without asking (the script checks the
  process's working directory or the container's compose project).
- Keep preview ids stable: the profile holds them. Do not pass `--id` unless the owner asks.
- Do not open a browser on the Mac yourself; the phone's Previews screen does that.

## Report

Tell the owner in a few lines:
- the preview name and status (for example "my-app is running"), and whether it was already
  running (then there is no new "Preview ready" notification);
- that it is on the phone under **Previews** and opens in the Mac browser;
- the ports registered with their labels (for example `8086 web, 8087 api, 8090 collaboration,
  9000 storage`), and any second preview;
- anything you fixed, installed or freed, and anything still not answering;
- Hako's message when there is one (for example that remote access is off, which means only the
  Mac can open it until the owner turns remote access on).

## Other commands

- `status` (this project) or `status --all`; `logs [--lines N]`; `wait`.
- `stop`, `restart`, `remove` (this project's previews, or `--id <id>`). Stopping the main preview
  of a multi-app script stops all its processes; an attached preview is only no longer followed.
- `detect [--fresh]`: show the plan without starting; `start --fresh`: detect again, keeping the id.
- `profile show|path|forget|set|note`, `env`.

Profiles live in `~/.hako/previews/profiles/<project>-<hash>.json`; `reference/frameworks.md` has
the framework table (dev commands, port flags, defaults, quirks) the script follows.
