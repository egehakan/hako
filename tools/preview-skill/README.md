# `/preview`: a Claude Code skill for Hako previews

Asked to "preview this", "let me see it" or "show me the app on my phone" inside a Hako terminal,
this Claude Code skill works out how the project runs, starts it as a Hako preview and waits until
it answers. The app then appears in Hako Remote under **Previews** and opens in the Mac browser (a
real Chrome on the Mac), so backends, cookies and sign-in work as they do on the Mac.

It handles single apps (Vite, Next.js, Astro, Nuxt, SvelteKit, Remix/React Router, Angular,
Gatsby, Create React App, Eleventy, Node servers, Django, Flask, FastAPI, Rails, static HTML,
`docker compose`) and projects that start several apps with one script (`dev.sh`, a root `dev`
script running turbo or concurrently, `Procfile.dev`, `make dev`). For those it registers every
port the browser uses on one preview. A second web app started by the same script, such as an
admin app, gets a preview of its own.

The first run in a project detects how it runs and saves a profile. Later runs reuse the profile:
about 0.1 s when the preview is already running, a few seconds when it is not. When a start fails,
the skill reads the server's output, fixes what it safely can (missing dependencies, Docker not
running, leftover servers of the same project on its ports, a renamed script), retries, and
records the fix in the profile. Lessons that apply to other projects go into `LEARNINGS.md`.

## Install

```sh
cp -R tools/preview-skill ~/.claude/skills/preview
```

To update an installed copy without losing your local learnings:

```sh
rsync -a --exclude LEARNINGS.md tools/preview-skill/ ~/.claude/skills/preview/
```

Requirements: Node 18 or later (no npm packages), and a Hako build with previews. Hako exports
`HAKO_DATA_DIR` and `HAKO_PREVIEWS=1` into its shells. Outside Hako the skill says so and offers
to run the dev server in the current terminal.

The script's own commands are pre-approved through the skill's `allowed-tools`. Claude Code may
still ask once for permission to use the skill itself. Non-interactive runs (`claude -p`) need
`--allowedTools "Skill(preview)"` or a matching allow rule.

## Files

| Path | What it is |
|---|---|
| `SKILL.md` | The instructions Claude follows: when to use the skill, the steps, what to do for each outcome, how to fix and record failures, and what to report |
| `scripts/preview.mjs` | Detection, profiles, port checks, request files for Hako, waiting on Hako's state file, log reading and diagnosis, and a small static server. Run `node scripts/preview.mjs help` for its commands |
| `reference/frameworks.md` | Framework table: how each one is recognised, its dev command, port flag, default port and quirks |
| `LEARNINGS.md` | Header only. Entries are added locally, see below |

## What stays local

These files contain local paths and are never committed to any repository:

- `~/.claude/skills/preview/LEARNINGS.md` entries, which the skill adds as it learns.
- Profiles, one per project, in `~/.hako/previews/profiles/<project>-<hash>.json`.
- Server logs in `~/.hako/previews/logs/<preview id>.log`. Each command the skill starts writes
  its output there as well as to its `Preview: <name>` tab, so the skill can read it after a
  failure.
