#!/usr/bin/env node
// Hako previews: finds how a project runs, starts it as a Hako preview, waits for it, and keeps a
// per-project profile so the next run is fast. No dependencies; Node 18+.
//
// Usage: node preview.mjs <command> [options]   (run `node preview.mjs help`)

import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SKILL_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SELF = fileURLToPath(import.meta.url);
const HOME = os.homedir();
const STORE = process.env.PREVIEW_SKILL_HOME || path.join(HOME, '.hako', 'previews');
const PROFILES = path.join(STORE, 'profiles');
const LOGS = path.join(STORE, 'logs');
const LEARNINGS = path.join(SKILL_DIR, 'LEARNINGS.md');
const DATA_DIR = process.env.HAKO_DATA_DIR || '';
const IN_HAKO = process.env.HAKO_PREVIEWS === '1' && DATA_DIR !== '';
const RESERVED = ['www', 'api', 'app', 'admin', 'relay', 'hako', 'mail', 'static', 'assets'];
const DEFAULT_WAIT_S = 110;

// ---------------------------------------------------------------------------------------------
// Small helpers

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const exists = (p) => { try { fs.accessSync(p); return true; } catch { return false; } };
const isDir = (p) => { try { return fs.statSync(p).isDirectory(); } catch { return false; } };
const readText = (p) => { try { return fs.readFileSync(p, 'utf8'); } catch { return null; } };
const readJson = (p) => { try { return JSON.parse(fs.readFileSync(p, 'utf8')); } catch { return null; } };
const sha = (s) => crypto.createHash('sha1').update(s).digest('hex');
const now = () => new Date().toISOString();
const q = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
const inside = (child, parent) => {
  const rel = path.relative(parent, child);
  return rel === '' || (!rel.startsWith('..') && !path.isAbsolute(rel));
};
const tilde = (p) => (p && p.startsWith(HOME) ? '~' + p.slice(HOME.length) : p);
const stripAnsi = (s) => s.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '').replace(/\x1b\][^\x07]*\x07/g, '').replace(/\r(?!\n)/g, '\n');

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, {
    encoding: 'utf8', timeout: opts.timeout ?? 10000, cwd: opts.cwd, env: opts.env ?? process.env,
    maxBuffer: 32 * 1024 * 1024,
  });
  return { ok: r.status === 0, status: r.status, out: (r.stdout || '').trim(), err: (r.stderr || '').trim(), error: r.error };
}
const which = (bin) => run('/bin/sh', ['-c', `command -v ${bin}`]).out || null;
// GIT_OPTIONAL_LOCKS=0: `git status` must not rewrite the index of the owner's repositories.
const git = (dir, ...args) => run('git', ['-C', dir, ...args], { timeout: 8000, env: { ...process.env, GIT_OPTIONAL_LOCKS: '0' } });

function writeJsonAtomic(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = path.join(path.dirname(file), `.${path.basename(file)}.${process.pid}.tmp`);
  fs.writeFileSync(tmp, JSON.stringify(value, null, 2) + '\n');
  fs.renameSync(tmp, file);
}

function parseArgs(argv) {
  const pos = []; const opt = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) {
      const eq = a.indexOf('=');
      if (eq > 0) { opt[a.slice(2, eq)] = a.slice(eq + 1); continue; }
      const key = a.slice(2); const next = argv[i + 1];
      if (next !== undefined && !next.startsWith('--')) { opt[key] = next; i++; } else opt[key] = true;
    } else pos.push(a);
  }
  return { pos, opt };
}

class Stop extends Error {
  constructor(code, message) { super(message); this.code = code; }
}

function slug(text, max = 40) {
  let s = String(text || '').toLowerCase().replace(/^@[^/]+\//, '').replace(/[^a-z0-9]+/g, '-').replace(/-+/g, '-').replace(/^-|-$/g, '');
  s = s.slice(0, max).replace(/-+$/g, '');
  return s || 'preview';
}
function previewId(text) {
  let id = slug(String(text).replace(/[-_.](monorepo|mono|repo|workspace|root)$/i, ''));
  if (RESERVED.includes(id)) id = `${id}-preview`;
  return id.slice(0, 40).replace(/-+$/, '');
}
const projectKey = (abs) => `${slug(path.basename(abs), 30)}-${sha(abs).slice(0, 8)}`;

// ---------------------------------------------------------------------------------------------
// Frameworks. Mirrors reference/frameworks.md.

const FRAMEWORKS = [
  { key: 'next', name: 'Next.js', deps: ['next'], port: 3000, args: '-p {port}', kind: 'frontend' },
  { key: 'nuxt', name: 'Nuxt', deps: ['nuxt'], port: 3000, args: '--port {port}', kind: 'frontend' },
  { key: 'sveltekit', name: 'SvelteKit', deps: ['@sveltejs/kit'], port: 5173, args: '--port {port} --strictPort', kind: 'frontend' },
  { key: 'astro', name: 'Astro', deps: ['astro'], port: 4321, args: '--port {port}', kind: 'frontend' },
  { key: 'react-router', name: 'React Router', deps: ['@react-router/dev'], port: 5173, args: '--port {port}', kind: 'frontend' },
  { key: 'remix', name: 'Remix', deps: ['@remix-run/dev'], port: 5173, args: '--port {port}', kind: 'frontend' },
  { key: 'angular', name: 'Angular', deps: ['@angular/cli'], port: 4200, args: '--port {port}', kind: 'frontend' },
  { key: 'gatsby', name: 'Gatsby', deps: ['gatsby'], port: 8000, args: '-p {port}', kind: 'frontend' },
  { key: 'cra', name: 'Create React App', deps: ['react-scripts'], port: 3000, portEnv: ['PORT'], env: { BROWSER: 'none' }, kind: 'frontend' },
  { key: 'vite', name: 'Vite', deps: ['vite'], port: 5173, args: '--port {port} --strictPort', kind: 'frontend' },
  { key: 'eleventy', name: 'Eleventy', deps: ['@11ty/eleventy'], port: 8080, args: '--port={port}', kind: 'frontend' },
  { key: 'nest', name: 'NestJS', deps: ['@nestjs/core'], port: 3000, portEnv: ['PORT'], kind: 'backend' },
  { key: 'node-server', name: 'Node server', deps: ['hono', 'express', 'fastify', 'koa', '@hocuspocus/server', 'ws'], port: 3000, portEnv: ['PORT'], kind: 'backend' },
];
const PY_FRAMEWORKS = { django: 8000, flask: 5000, fastapi: 8000 };

// ---------------------------------------------------------------------------------------------
// Package managers

function packageManager(dir, root) {
  for (const d of [dir, root].filter(Boolean)) {
    const pkg = readJson(path.join(d, 'package.json'));
    const field = pkg?.packageManager?.split('@')[0];
    if (field && PM[field]) return field;
    if (exists(path.join(d, 'bun.lock')) || exists(path.join(d, 'bun.lockb'))) return 'bun';
    if (exists(path.join(d, 'pnpm-lock.yaml'))) return 'pnpm';
    if (exists(path.join(d, 'yarn.lock'))) return 'yarn';
    if (exists(path.join(d, 'package-lock.json'))) return 'npm';
  }
  return 'npm';
}
const PM = {
  // npm needs `--` before arguments meant for the script; pnpm, yarn and bun pass them through and
  // would hand a literal `--` to the script.
  npm: { install: 'npm install', run: (s, a) => `npm run ${s}${a ? ` -- ${a}` : ''}`, exec: 'npx --no-install' },
  pnpm: { install: 'pnpm install', run: (s, a) => `pnpm run ${s}${a ? ` ${a}` : ''}`, exec: 'pnpm exec' },
  yarn: { install: 'yarn install', run: (s, a) => `yarn run ${s}${a ? ` ${a}` : ''}`, exec: 'yarn' },
  bun: { install: 'bun install', run: (s, a) => `bun run ${s}${a ? ` ${a}` : ''}`, exec: 'bun x' },
};

// ---------------------------------------------------------------------------------------------
// Ports and processes

function listeners() {
  const map = new Map();
  const r = run('lsof', ['-nP', '-iTCP', '-sTCP:LISTEN', '-Fpn'], { timeout: 10000 });
  let pid = null;
  for (const line of r.out.split('\n')) {
    if (line.startsWith('p')) pid = Number(line.slice(1));
    else if (line.startsWith('n') && pid) {
      const port = Number(line.slice(line.lastIndexOf(':') + 1));
      if (!port) continue;
      if (!map.has(port)) map.set(port, new Set());
      map.get(port).add(pid);
    }
  }
  return map;
}

function procInfo(pid) {
  const command = run('ps', ['-o', 'command=', '-p', String(pid)]).out;
  const cwdLine = run('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn']).out.split('\n').find((l) => l.startsWith('n'));
  return { pid, command, cwd: cwdLine ? cwdLine.slice(1) : null };
}

function connects(port, host, ms = 400) {
  return new Promise((resolve) => {
    const s = net.connect({ port, host });
    const done = (v) => { s.destroy(); resolve(v); };
    s.setTimeout(ms, () => done(false));
    s.once('connect', () => done(true));
    s.once('error', () => done(false));
  });
}
const portOpen = async (port) => (await connects(port, '127.0.0.1')) || (await connects(port, '::1'));

function httpAnswers(port, p = '/', ms = 3000) {
  const ask = (host) => new Promise((resolve) => {
    const req = http.get({ host, port, path: p, timeout: ms, headers: { 'user-agent': 'hako-preview-skill' } }, (res) => { res.resume(); resolve(res.statusCode); });
    req.on('timeout', () => { req.destroy(); resolve(null); });
    req.on('error', () => resolve(null));
  });
  return ask('127.0.0.1').then((s) => s ?? ask('::1'));
}

let dockerCache;
function dockerContainers() {
  if (dockerCache) return dockerCache;
  const r = run('docker', ['ps', '--format', '{{.Names}}\t{{.Ports}}\t{{.Label "com.docker.compose.project.working_dir"}}'], { timeout: 8000 });
  dockerCache = r.ok ? r.out.split('\n').filter(Boolean).map((l) => {
    const [name, ports, workdir] = l.split('\t');
    const hostPorts = [...(ports || '').matchAll(/:(\d+)->/g)].map((m) => Number(m[1]));
    return { name, hostPorts, workdir };
  }) : [];
  return dockerCache;
}
const dockerUp = () => run('docker', ['info', '--format', '{{.ServerVersion}}'], { timeout: 8000 }).ok;

/** Who listens on `port`: null when free. */
async function portHolder(port, root, L) {
  const pids = [...(L.get(port) || [])];
  if (!pids.length) {
    return (await portOpen(port)) ? { kind: 'other', desc: 'a process of another user or the system' } : null;
  }
  const infos = pids.map(procInfo);
  const docker = infos.find((i) => /docker|vpnkit|OrbStack|colima|limactl|podman/i.test(i.command));
  if (docker) {
    const c = dockerContainers().find((c) => c.hostPorts.includes(port));
    if (c?.workdir && inside(c.workdir, root)) return { kind: 'project-docker', desc: `container ${c.name} of this project` };
    return { kind: 'docker', desc: c ? `Docker container ${c.name}${c.workdir ? ` (from ${tilde(c.workdir)})` : ''}` : 'Docker' };
  }
  const mine = infos.filter((i) => i.cwd && inside(i.cwd, root));
  if (mine.length) return { kind: 'project', pids: mine.map((i) => i.pid), desc: mine.map(describeProc).join('; ') };
  return { kind: 'other', pids, desc: infos.map(describeProc).join('; ') };
}
const describeProc = (i) => `pid ${i.pid} \`${i.command.slice(0, 90)}\`${i.cwd ? ` in ${tilde(i.cwd)}` : ''}`;

async function nextFreePort(from, L, avoid = new Set()) {
  for (let p = from; p < from + 200; p++) {
    if (avoid.has(p) || L.has(p)) continue;
    if (!(await portOpen(p))) return p;
  }
  throw new Stop(1, `No free port found between ${from} and ${from + 200}`);
}

// ---------------------------------------------------------------------------------------------
// Hako state and requests

const requestsDir = () => path.join(DATA_DIR, 'previews', 'requests');
const statePath = () => path.join(DATA_DIR, 'previews', 'state.json');
const readState = () => readJson(statePath()) || { previews: [], rejected: [], updated: 0 };

function sendRequest(request) {
  const dir = requestsDir();
  fs.mkdirSync(dir, { recursive: true });
  const name = crypto.randomUUID();
  const tmp = path.join(dir, `.${name}.json.tmp`);
  fs.writeFileSync(tmp, JSON.stringify(request, null, 2));
  const file = path.join(dir, `${name}.json`);
  fs.renameSync(tmp, file);
  return { file, name: `${name}.json`, sent: Date.now() };
}

/** Waits for Hako to take the request, then for `done(preview)` to hold. */
async function waitFor(id, req, timeoutS, done, onTick) {
  const t0 = Date.now();
  let taken = false; let lastTick = Date.now();
  for (;;) {
    if (!taken && !exists(req.file)) taken = true;
    if (!taken && Date.now() - req.sent > 10000) {
      throw new Stop(1, `Hako did not pick up the request within 10 s (${tilde(req.file)} is still there). Is the Hako app that owns HAKO_DATA_DIR=${tilde(DATA_DIR)} running, and is it a build with previews? Remove that file and retry once Hako runs.`);
    }
    const st = readState();
    const rejected = (st.rejected || []).find((r) => r.file === req.name || (r.file || '').endsWith(req.name));
    if (rejected) throw new Stop(1, `Hako rejected the request: ${rejected.message}`);
    const p = (st.previews || []).find((x) => x.id === id);
    if (taken && (st.updated || 0) >= req.sent && done(p)) return { preview: p, seconds: (Date.now() - t0) / 1000 };
    if (Date.now() - t0 > timeoutS * 1000) return { preview: p, timeout: true, seconds: (Date.now() - t0) / 1000 };
    if (onTick && Date.now() - lastTick > 10000) { lastTick = Date.now(); onTick(p, (Date.now() - t0) / 1000); }
    await sleep(400);
  }
}

// ---------------------------------------------------------------------------------------------
// Profiles

const profilePath = (root) => path.join(PROFILES, `${projectKey(root)}.json`);
const draftPath = (root) => path.join(PROFILES, `${projectKey(root)}.draft.json`);
const loadProfile = (root) => readJson(profilePath(root));
const loadDraft = (root) => readJson(draftPath(root));

function fingerprint(root, files) {
  return sha(files.map((f) => `${f}\n${readText(path.join(root, f)) ?? '<missing>'}`).join('\n')).slice(0, 12);
}

function allProfiles() {
  try {
    return fs.readdirSync(PROFILES).filter((f) => f.endsWith('.json') && !f.endsWith('.draft.json')).map((f) => readJson(path.join(PROFILES, f))).filter(Boolean);
  } catch { return []; }
}

// ---------------------------------------------------------------------------------------------
// Finding the project

const PROJECT_MARKERS = ['package.json', 'manage.py', 'requirements.txt', 'pyproject.toml', 'Gemfile', 'index.html', 'docker-compose.yml', 'compose.yaml', 'compose.yml', 'Procfile.dev', 'Makefile', 'go.mod', 'Cargo.toml'];
const STATIC_DIRS = ['.', 'public', 'site', 'docs', 'dist', 'build'];
const hasMarkers = (dir) => PROJECT_MARKERS.some((m) => exists(path.join(dir, m))) || STATIC_DIRS.some((d) => exists(path.join(dir, d, 'index.html')));

function repoActivity(dir) {
  const status = git(dir, 'status', '--porcelain');
  const changed = status.ok ? status.out.split('\n').filter(Boolean).map((l) => l.slice(3).replace(/^"|"$/g, '').split(' -> ').pop()) : [];
  const recent = git(dir, 'log', '--since=1.hour', '--pretty=format:%h');
  const commits = recent.ok ? recent.out.split('\n').filter(Boolean).length : 0;
  let newest = 0;
  for (const f of changed.slice(0, 200)) {
    try { newest = Math.max(newest, fs.statSync(path.join(dir, f)).mtimeMs); } catch {}
  }
  const head = git(dir, 'log', '-1', '--pretty=format:%ct');
  newest = Math.max(newest, head.ok ? Number(head.out) * 1000 : 0);
  return { dir, changed: changed.length, commits, newest, active: changed.length > 0 || commits > 0 };
}

/** Changed files (uncommitted, and committed in the last hour), relative to `root`. */
function changedFiles(root) {
  const files = new Set();
  const s = git(root, 'status', '--porcelain');
  if (s.ok) s.out.split('\n').filter(Boolean).forEach((l) => files.add(l.slice(3).replace(/^"|"$/g, '').split(' -> ').pop()));
  const c = git(root, 'log', '--since=1.hour', '--name-only', '--pretty=format:');
  if (c.ok) c.out.split('\n').filter(Boolean).forEach((f) => files.add(f));
  return [...files];
}

/** Resolves the directory to work on: a repo, or one repo inside a folder of repos. */
function resolveRoot(start, opt) {
  start = path.resolve(start);
  if (!isDir(start)) throw new Stop(1, `Not a directory: ${start}`);
  const top = git(start, 'rev-parse', '--show-toplevel');
  if (top.ok) {
    const root = fs.realpathSync(top.out);
    const real = fs.realpathSync(start);
    return { root, repo: root, hint: real !== root ? real : null };
  }
  const real = fs.realpathSync(start);
  if (hasMarkers(start) || exists(profilePath(real))) return { root: real, repo: null, hint: null };
  // A folder of repos (or projects): pick the one that changed.
  const kids = [];
  for (const name of fs.readdirSync(start)) {
    if (name.startsWith('.') || name === 'node_modules') continue;
    const d = path.join(start, name);
    if (!isDir(d)) continue;
    if (exists(path.join(d, '.git'))) kids.push(repoActivity(d));
    else if (hasMarkers(d)) kids.push({ dir: d, changed: 0, commits: 0, newest: fs.statSync(d).mtimeMs, active: false });
  }
  if (!kids.length) throw new Stop(1, `No project found in ${start}: no package.json, manage.py, requirements.txt, pyproject.toml, Gemfile, index.html, compose file or Makefile, and no repositories in its subfolders. Look at the README and tell the script what to run with --command and --port.`);
  const active = kids.filter((k) => k.active).sort((a, b) => b.newest - a.newest);
  const pool = active.length ? active : [...kids].sort((a, b) => b.newest - a.newest);
  if (pool.length === 1 && (active.length === 1 || kids.length === 1)) {
    const d = fs.realpathSync(pool[0].dir);
    return { root: d, repo: exists(path.join(d, '.git')) ? d : null, hint: null, picked: `${path.basename(d)} (the only ${active.length ? 'repository with recent changes' : 'project'} in ${tilde(start)})` };
  }
  const lines = pool.slice(0, 8).map((k, i) => `  ${i + 1}. ${tilde(k.dir)}${k.active ? ` — ${k.changed} uncommitted file(s), ${k.commits} commit(s) in the last hour` : ''}${i === 0 ? '  (recommended: most recently changed)' : ''}`);
  throw new Stop(2, `ASK: Several projects in ${tilde(start)} could be meant${active.length ? ' (all have recent changes)' : ' (none has recent changes)'}:\n${lines.join('\n')}\nAsk the owner which one (unless you know which repo you changed in this session), then rerun with --dir <path>.`);
}

// ---------------------------------------------------------------------------------------------
// Detection

function deps(pkg) { return { ...(pkg?.dependencies || {}), ...(pkg?.devDependencies || {}) }; }

function workspaceDirs(root, pkg) {
  let patterns = Array.isArray(pkg?.workspaces) ? pkg.workspaces : pkg?.workspaces?.packages || [];
  const pnpmWs = readText(path.join(root, 'pnpm-workspace.yaml'));
  if (pnpmWs) {
    const block = pnpmWs.split(/\n(?=\S)/).find((b) => b.startsWith('packages:')) || '';
    patterns = patterns.concat([...block.matchAll(/^\s*-\s*['"]?([^'"\n#]+?)['"]?\s*$/gm)].map((m) => m[1]));
  }
  const dirs = new Set();
  for (const pat of patterns) {
    if (pat.startsWith('!')) continue;
    const clean = pat.replace(/\/\*\*$/, '/*');
    if (clean.endsWith('/*')) {
      const base = path.join(root, clean.slice(0, -2));
      try { for (const n of fs.readdirSync(base)) if (exists(path.join(base, n, 'package.json'))) dirs.add(path.join(base, n)); } catch {}
    } else if (exists(path.join(root, clean, 'package.json'))) dirs.add(path.join(root, clean));
  }
  return [...dirs].sort();
}

function firstMatch(text, regexes) {
  for (const re of regexes) { const m = text?.match(re); if (m) return Number(m[1]); }
  return null;
}
const SCRIPT_PORT_RES = [/(?:--port|-p|--http-port)(?:\s+|=)(\d{2,5})\b/, /\bPORT=(\d{2,5})\b/, /runserver\s+(?:[\d.]+:)?(\d{2,5})/];

function configPort(dir, fw) {
  const files = fs.readdirSync(dir).filter((f) => /^(vite|astro|nuxt|svelte|remix|react-router)\.config\.(c|m)?(js|ts)$/.test(f));
  for (const f of files) {
    const p = firstMatch(readText(path.join(dir, f)), [/\bport\s*:\s*(\d{2,5})\b/]);
    if (p) return { port: p, from: f };
  }
  if (fw?.key === 'angular') {
    const p = firstMatch(readText(path.join(dir, 'angular.json')), [/"port"\s*:\s*(\d{2,5})/]);
    if (p) return { port: p, from: 'angular.json' };
  }
  for (const f of ['.env.development.local', '.env.local', '.env.development', '.env', '.env.example']) {
    const p = firstMatch(readText(path.join(dir, f)), [/^\s*PORT\s*=\s*["']?(\d{2,5})/m]);
    if (p) return { port: p, from: f };
  }
  return null;
}

function sourcePort(dir) {
  for (const f of ['src/index.ts', 'src/index.js', 'src/server.ts', 'src/server.js', 'src/main.ts', 'src/main.js', 'src/app.ts', 'index.ts', 'index.js', 'server.ts', 'server.js', 'app.js', 'main.ts']) {
    const t = readText(path.join(dir, f));
    const p = firstMatch(t, [/PORT\s*(?:\|\||\?\?)\s*['"]?(\d{2,5})/, /\.listen\(\s*(\d{2,5})/, /\bport\s*:\s*(\d{4,5})\b/]);
    if (p) return { port: p, from: f };
  }
  return null;
}

function analyzeNodeApp(dir, root, pm) {
  const pkg = readJson(path.join(dir, 'package.json')) || {};
  const d = deps(pkg);
  const fw = FRAMEWORKS.find((f) => f.deps.some((dep) => dep in d)) || null;
  const scripts = pkg.scripts || {};
  const scriptName = pickDevScript(scripts);
  const script = scriptName ? scripts[scriptName] : null;
  const scriptPort = firstMatch(script, SCRIPT_PORT_RES);
  const conf = configPort(dir, fw);
  const src = fw?.kind === 'backend' || !fw ? sourcePort(dir) : null;
  let port = scriptName ? scriptPort || conf?.port || src?.port || null : null;
  let portFrom = scriptPort ? `the ${scriptName} script` : conf?.from || src?.from || null;
  if (!port && fw && scriptName) { port = fw.port; portFrom = `${fw.name} default`; }
  if (fw?.key === 'remix' && !readdirSafe(dir).some((f) => /^vite\.config\./.test(f)) && !scriptPort) { port = 3000; portFrom = 'Remix (classic) default'; }
  return {
    dir, rel: path.relative(root, dir) || '.', name: pkg.name || path.basename(dir), base: path.basename(dir),
    pkg, fw, scriptName, script, scriptPort, port, portFrom, pm, scripts,
  };
}
// A script that runs a dev server, by name or by what it runs.
const DEV_RE = /\b(?:next|nuxi?|astro|remix|react-router|ng|gatsby|eleventy|svelte-kit|vite|webpack|parcel|rsbuild|wrangler)\s+(?:dev|serve|develop)\b|(?:^|[\s;&])vite(?:\s+--|\s*$)|react-scripts start|tsx watch|node --watch|nodemon|bun --(?:watch|hot)|flask run|runserver/;
function pickDevScript(scripts) {
  if (scripts.dev) return 'dev';
  const names = Object.keys(scripts);
  const byValue = names.filter((n) => DEV_RE.test(scripts[n])).sort((a, b) => Number(!a.startsWith('dev')) - Number(!b.startsWith('dev')) || a.length - b.length);
  return byValue[0] || ['develop', 'serve', 'start'].find((n) => scripts[n]) || null;
}
const readdirSafe = (d) => { try { return fs.readdirSync(d); } catch { return []; } };

function findVenv(dir) {
  for (const v of ['.venv', 'venv', 'env', '.env-py']) if (exists(path.join(dir, v, 'bin', 'python'))) return path.join(dir, v);
  return null;
}

function analyzePython(root) {
  const req = (readText(path.join(root, 'requirements.txt')) || '') + (readText(path.join(root, 'pyproject.toml')) || '') + (readText(path.join(root, 'Pipfile')) || '');
  const venv = findVenv(root);
  // Without a virtualenv the install step creates .venv, so commands point there either way.
  const venvRel = venv ? path.relative(root, venv) : '.venv';
  const py = `${venvRel}/bin/python`;
  const bin = (b) => `${venvRel}/bin/${b}`;
  const install = exists(path.join(root, 'requirements.txt'))
    ? 'python3 -m venv .venv && .venv/bin/pip install -r requirements.txt'
    : 'python3 -m venv .venv && .venv/bin/pip install -e .';
  const env = { PYTHONUNBUFFERED: '1' };
  if (exists(path.join(root, 'manage.py'))) {
    return { framework: 'django', name: 'Django', command: `${py} manage.py runserver`, args: '127.0.0.1:{port}', port: PY_FRAMEWORKS.django, env, venv, install, deps: /django/i.test(req) };
  }
  if (/\bflask\b/i.test(req)) {
    const app = ['app.py', 'wsgi.py', 'main.py', 'server.py', 'application.py'].find((f) => exists(path.join(root, f)));
    const appArg = app ? ` --app ${app.replace(/\.py$/, '')}` : '';
    return { framework: 'flask', name: 'Flask', command: `${bin('flask')}${appArg} run --debug`, args: '--port {port}', port: PY_FRAMEWORKS.flask, env, venv, install, deps: true };
  }
  if (/\bfastapi\b/i.test(req)) {
    const file = ['main.py', 'app.py', 'app/main.py', 'src/main.py'].find((f) => /FastAPI\(/.test(readText(path.join(root, f)) || ''));
    const mod = file ? file.replace(/\.py$/, '').replace(/\//g, '.') : 'main';
    return { framework: 'fastapi', name: 'FastAPI', command: `${bin('uvicorn')} ${mod}:app --reload`, args: '--port {port}', port: PY_FRAMEWORKS.fastapi, env, venv, install, deps: true };
  }
  return null;
}

function parseCompose(root) {
  const file = ['docker-compose.yml', 'docker-compose.yaml', 'compose.yaml', 'compose.yml'].find((f) => exists(path.join(root, f)));
  if (!file) return null;
  const text = readText(path.join(root, file));
  const services = [];
  let inServices = false; let cur = null; let inPorts = false; let indent = null;
  for (const raw of text.split('\n')) {
    const line = raw.replace(/\s+#.*$/, '');
    if (/^services:\s*$/.test(line)) { inServices = true; continue; }
    if (/^\S/.test(line)) { inServices = false; continue; }
    if (!inServices || !line.trim()) continue;
    const ind = line.match(/^(\s*)/)[1].length;
    if (indent === null) indent = ind;
    const svc = line.match(/^\s*([A-Za-z0-9_.-]+):\s*$/);
    if (svc && ind === indent) { cur = { name: svc[1], image: '', ports: [] }; services.push(cur); inPorts = false; continue; }
    if (!cur) continue;
    const img = line.match(/^\s*image:\s*["']?([^"'\s]+)/);
    if (img) cur.image = img[1];
    if (/^\s*ports:\s*$/.test(line)) { inPorts = true; continue; }
    const port = line.match(/^\s*-\s*["']?(?:[\d.]+:)?(\d+):(\d+)/);
    if (inPorts && port) { cur.ports.push({ host: Number(port[1]), container: Number(port[2]) }); continue; }
    if (inPorts && /^\s*[A-Za-z_]+:/.test(line)) inPorts = false;
  }
  return { file, services };
}

/** Infrastructure the browser talks to directly (object storage links, mail catchers). */
function browserFacing(svc) {
  const id = `${svc.name} ${svc.image}`;
  if (/minio|localstack|s3mock|azurite|fake-gcs/i.test(id)) {
    const p = svc.ports.find((p) => [9000, 4566, 10000, 4443].includes(p.container)) || svc.ports[0];
    return p ? { port: p.host, label: 'storage' } : null;
  }
  if (/mailpit|mailhog|maildev/i.test(id)) {
    const p = svc.ports.find((p) => [8025, 1080].includes(p.container));
    return p ? { port: p.host, label: 'mail' } : null;
  }
  return null;
}

const FRONT_NAMES = ['web', 'app', 'frontend', 'client', 'site', 'www', 'ui', 'dashboard'];
const WORKER_RE = /worker|cron|queue|jobs?$|scheduler|consumer/i;

function backendLabel(app) {
  if (/^(server|api|backend)$|(^|-)api$/i.test(app.base)) return 'api';
  return slug(app.base, 30);
}

function rankFront(a) {
  const i = FRONT_NAMES.indexOf(a.base.toLowerCase());
  return (i >= 0 ? i : 50) + (/admin|docs|storybook|playground|example/i.test(a.base) ? 100 : 0);
}

/** The project's own way to run everything at once, if there is one. */
function findOrchestrator(root, pkg, pm, apps) {
  const rootDev = pkg?.scripts?.dev;
  const fileCmd = (f) => {
    const abs = path.join(root, f);
    let executable = false;
    try { fs.accessSync(abs, fs.constants.X_OK); executable = true; } catch {}
    const shebang = (readText(abs) || '').startsWith('#!/usr/bin/env bash') || (readText(abs) || '').includes('bash') ? 'bash' : 'sh';
    return executable ? `./${f}` : `${shebang} ${f}`;
  };
  if (rootDev) {
    const m = rootDev.trim().match(/^(?:(?:ba)?sh\s+)?(?:\.\/)?([\w./-]+\.sh)$/);
    if (m && exists(path.join(root, m[1]))) return { kind: 'script', file: m[1], command: fileCmd(m[1]), text: readText(path.join(root, m[1])) };
    if (apps.length && /turbo|nx\b|concurrently|npm-run-all|run-p\b|lerna|--parallel|--filter|-r\s|--recursive|--workspaces|foreman|overmind|&/.test(rootDev)) {
      return { kind: 'script', file: 'package.json', command: PM[pm].run('dev'), text: rootDev };
    }
  }
  for (const f of ['dev.sh', 'scripts/dev.sh', 'start-dev.sh', 'run-dev.sh', 'bin/dev']) {
    if (exists(path.join(root, f)) && !isDir(path.join(root, f))) return { kind: 'script', file: f, command: fileCmd(f), text: readText(path.join(root, f)) + (exists(path.join(root, 'Procfile.dev')) ? '\n' + readText(path.join(root, 'Procfile.dev')) : '') };
  }
  if (exists(path.join(root, 'Procfile.dev'))) {
    const tool = ['overmind', 'hivemind', 'foreman', 'honcho'].find(which);
    const text = readText(path.join(root, 'Procfile.dev'));
    if (tool) return { kind: 'script', file: 'Procfile.dev', command: `${tool} start -f Procfile.dev`, text };
    return { kind: 'missing-tool', file: 'Procfile.dev', text, message: 'Procfile.dev found but none of overmind, hivemind, foreman, honcho is installed' };
  }
  const mk = readText(path.join(root, 'Makefile'));
  if (mk && /^dev:/m.test(mk)) {
    const body = mk.split(/^dev:.*$/m)[1]?.split(/^\S/m)[0] || '';
    return { kind: 'script', file: 'Makefile', command: 'make dev', text: body };
  }
  return null;
}

function appsStartedBy(orch, apps) {
  const t = orch.text || '';
  if (/turbo(\s+run)?\s+dev|nx\s+run-many|--parallel|-r\s+dev|--recursive|--workspaces/.test(t) && !/--filter/.test(t)) return apps.filter((a) => a.scripts.dev);
  const named = apps.filter((a) => {
    const names = [a.rel, a.base, a.name].filter(Boolean).map((n) => n.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    return names.some((n) => new RegExp(`(^|[^\\w-])${n}([^\\w-]|$)`).test(t));
  });
  return named;
}

/** Ports per app found in the orchestrator's text: a line naming exactly one app and a port. */
function portsFromText(text, apps) {
  const out = new Map();
  for (const line of (text || '').split('\n')) {
    const ports = [...line.matchAll(/(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):(\d{2,5})/g), ...line.matchAll(/(?:--port|-p|PORT=)\s*=?\s*(\d{2,5})\b/g)].map((m) => Number(m[1]));
    if (!ports.length) continue;
    const hits = apps.filter((a) => [a.base, a.name, a.rel].some((n) => n && new RegExp(`(^|[^\\w-])${n.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}([^\\w-]|$)`).test(line)));
    if (hits.length === 1 && !out.has(hits[0].dir)) out.set(hits[0].dir, ports[0]);
  }
  return out;
}

function singleAppPreview(app, idBase, root) {
  const fw = app.fw;
  const pmx = PM[app.pm];
  const p = {
    id: idBase, name: idBase, action: 'start', directory: app.dir, framework: fw?.key || 'node', label: fw?.kind === 'backend' ? backendLabel(app) : 'web',
    port: app.port, ports: [], env: { ...(fw?.env || {}) }, ready_path: '/', url_path: '/', needs: [],
    port_fixed: false, port_from: app.portFrom,
  };
  if (!app.scriptName) return { ...p, command: null, problem: `${tilde(app.dir)}/package.json has no dev, start, develop or serve script` };
  if (app.scriptPort) {
    // The script fixes the port. Run it as is; a busy port is handled at start time.
    p.command = pmx.run(app.scriptName);
    p.port_fixed = true;
    p.script_port = { script: app.script, exec: pmx.exec, port: app.scriptPort };
  } else if (fw?.portEnv) {
    p.command = pmx.run(app.scriptName);
    p.port_env = fw.portEnv;
  } else if (fw?.args) {
    p.command = pmx.run(app.scriptName);
    p.port_args = fw.args;
  } else {
    p.command = pmx.run(app.scriptName);
    p.port_env = ['PORT'];
  }
  if (!p.port) p.problem = `could not tell which port ${app.name} listens on`;
  return p;
}

/** Builds a plan: the previews to run for the project at `root`. */
function detect(root, repo, hint, opt) {
  const notes = []; const questions = [];
  const pkg = readJson(path.join(root, 'package.json'));
  const pm = pkg ? packageManager(root, root) : null;
  const relies = [];
  const plan = {
    version: 1, key: projectKey(root), project: root, repo, created: now(), updated: now(),
    package_manager: pm, install: null, relies_on: relies, previews: [], notes: [], detected: {},
    last_success: null, last_failure: null, last_start_seconds: null,
  };
  const idBase = previewId(opt.id || (pkg?.name && !pkg.name.startsWith('@') && pkg.name !== 'root' ? pkg.name : path.basename(root)));

  if (pkg) {
    relies.push('package.json');
    plan.install = { command: PM[pm].install, dir: root, marker: 'node_modules' };
    const wsDirs = workspaceDirs(root, pkg);
    const apps = wsDirs.map((d) => analyzeNodeApp(d, root, pm));
    const orch = opt.app ? null : findOrchestrator(root, pkg, pm, apps);
    if (orch?.kind === 'missing-tool') { notes.push(orch.message); }
    if (orch?.kind === 'script') {
      if (orch.file !== 'package.json') relies.push(orch.file);
      // Packages without a dev or start script are libraries, not servers.
      const servers = apps.filter((a) => a.scriptName);
      const started = appsStartedBy(orch, servers);
      const textPorts = portsFromText(orch.text, servers);
      for (const a of started) if (textPorts.has(a.dir) && (!a.scriptPort || a.scriptPort === textPorts.get(a.dir))) { a.port = textPorts.get(a.dir); a.portFrom = orch.file; }
      // An app the dev script gives a port to but with no framework dependency is a plain server.
      for (const a of started) if (!a.fw && textPorts.has(a.dir)) a.fw = { key: 'node', name: 'Node server', kind: 'backend' };
      const withPorts = started.filter((a) => a.port && a.fw);
      const fronts = withPorts.filter((a) => a.fw.kind === 'frontend').sort((a, b) => rankFront(a) - rankFront(b) || a.port - b.port);
      const backs = withPorts.filter((a) => a.fw.kind === 'backend' && !WORKER_RE.test(a.base)).sort((a, b) => a.port - b.port);
      const main = fronts[0] || backs[0];
      if (main) {
        const needs = /docker(-compose|\s+compose|\s)/.test(orch.text || '') ? ['docker'] : [];
        const secondaries = fronts.slice(1);
        const partnerOf = (b) => secondaries.find((s) => b.base.startsWith(`${s.base}-`) || b.base.startsWith(`${s.base}_`));
        const mainExtras = backs.filter((b) => b !== main && !partnerOf(b)).map((b) => ({ port: b.port, label: backendLabel(b) }));
        const compose = parseCompose(root);
        if (compose) {
          const up = (orch.text || '').match(/(?:docker[ -]compose|docker_compose)\s+(?:-f\s+\S+\s+)?up\b([^\n;&|]*)/);
          const names = up ? up[1].split(/\s+/).filter((w) => w && !w.startsWith('-')) : null;
          for (const svc of compose.services) {
            if (names && names.length && !names.includes(svc.name)) continue;
            if (!up && !needs.length) continue;
            const bf = browserFacing(svc);
            if (bf) mainExtras.push(bf);
          }
          relies.push(compose.file);
        }
        plan.detected = { kind: 'orchestrator', how: `${orch.command} (${orch.file})`, apps: started.filter((a) => a.fw || a.port).map((a) => `${a.rel}${a.fw ? ` [${a.fw.name}]` : ''}${a.port ? ` :${a.port}` : ''}`) };
        plan.previews.push({
          id: idBase, name: idBase, action: 'start', directory: root, command: orch.command, framework: main.fw.key,
          port: main.port, label: main.fw.kind === 'frontend' ? 'web' : backendLabel(main), ports: dedupePorts(mainExtras, main.port),
          env: {}, ready_path: '/', url_path: '/', needs, port_fixed: true, port_from: main.portFrom,
        });
        for (const s of secondaries) {
          const partners = backs.filter((b) => partnerOf(b) === s).map((b) => ({ port: b.port, label: backendLabel(b) }));
          plan.previews.push({
            id: previewId(`${idBase}-${s.base}`), name: `${idBase}-${s.base}`, action: 'attach', after: idBase, directory: root, command: null,
            framework: s.fw.key, port: s.port, label: 'web', ports: dedupePorts(partners, s.port), url_path: '/', ready_path: '/', needs: [], port_fixed: true, port_from: s.portFrom,
          });
        }
        const unknown = started.filter((a) => a.fw && !a.port && !WORKER_RE.test(a.base));
        if (unknown.length) notes.push(`Ports not found for: ${unknown.map((a) => a.rel).join(', ')} (add them with \`profile set\` if the browser uses them)`);
        const skipped = started.filter((a) => WORKER_RE.test(a.base) && a.port);
        if (skipped.length) notes.push(`Not registered (workers the browser does not call): ${skipped.map((a) => `${a.rel}:${a.port}`).join(', ')}`);
        plan.framework = main.fw.key;
        plan.notes = notes;
        return { plan, questions };
      }
      notes.push(`${orch.file} looks like the project's way to start everything, but no app with a known port was found in it`);
      if (!apps.length) {
        const ports = [...(orch.text || '').matchAll(/(?:localhost|127\.0\.0\.1):(\d{2,5})/g)].map((m) => Number(m[1]));
        if (ports.length) {
          plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: orch.command, framework: 'custom', port: ports[0], label: 'web', ports: dedupePorts(ports.slice(1).map((p) => ({ port: p })), ports[0]), env: {}, ready_path: '/', url_path: '/', needs: /docker/.test(orch.text) ? ['docker'] : [], port_fixed: true, port_from: orch.file });
          questions.push(`Confirm the main port: ${orch.file} mentions ${[...new Set(ports)].join(', ')}; ${ports[0]} was chosen.`);
          plan.notes = notes; plan.detected = { kind: 'orchestrator', how: orch.command };
          return { plan, questions };
        }
      }
    }

    // One app: the root package, or one workspace app.
    let app = null;
    const runnable = apps.filter((a) => a.fw && a.scriptName);
    if (opt.app) {
      app = apps.find((a) => [a.base, a.name, a.rel].includes(opt.app) || path.resolve(root, opt.app) === a.dir);
      if (!app) throw new Stop(1, `No workspace app named "${opt.app}". Apps: ${apps.map((a) => a.rel).join(', ')}`);
    } else if (hint) {
      app = apps.find((a) => inside(hint, a.dir)) || null;
    }
    if (!app && runnable.length) {
      const changed = changedFiles(root);
      const touched = runnable.filter((a) => changed.some((f) => inside(path.join(root, f), a.dir)));
      if (touched.length === 1) { app = touched[0]; notes.push(`Picked ${app.rel}: the only app with recent changes`); }
      else {
        const fronts = (touched.length ? touched : runnable).filter((a) => a.fw.kind === 'frontend').sort((a, b) => rankFront(a) - rankFront(b));
        if (fronts.length === 1) { app = fronts[0]; notes.push(`Picked ${app.rel}: the only web app${touched.length ? ' with recent changes' : ''}`); }
        else if (fronts.length > 1) {
          app = fronts[0];
          questions.push(`ASK: Which app? ${fronts.map((a, i) => `${a.rel} [${a.fw.name}]${i === 0 ? ' (recommended)' : ''}`).join(', ')}. Rerun with --app <dir>.`);
        }
      }
    }
    if (!app && !runnable.length && apps.length === 0) app = analyzeNodeApp(root, root, pm);
    if (!app && apps.length) {
      app = analyzeNodeApp(root, root, pm);
      if (!app.fw) throw new Stop(1, `A workspace without a root dev script and without a recognised web app in: ${apps.map((a) => a.rel).join(', ')}. Read the README to find out how it is run, then pass --command and --port.`);
    }
    if (app && (app.fw || app.scriptName)) {
      const id = app.dir === root ? idBase : previewId(app.fw?.kind === 'frontend' && rankFront(app) < 50 ? idBase : `${idBase}-${app.base}`);
      const p = singleAppPreview(app, opt.id ? previewId(opt.id) : id, root);
      if (app.dir !== root) relies.push(path.join(app.rel, 'package.json'));
      plan.previews.push(p);
      plan.framework = p.framework;
      plan.detected = { kind: 'app', how: `${app.fw?.name || 'package script'} in ${app.rel} (${pm})`, port_from: app.portFrom };
      // The frontend's backend: another app on a port the frontend's code calls.
      if (app.fw?.kind === 'frontend' && apps.length) {
        const calls = scanLocalhostPorts(app.dir);
        for (const b of apps.filter((a) => a !== app && a.port && calls.has(a.port))) {
          const bp = singleAppPreview(b, previewId(`${idBase}-${b.base}`), root);
          bp.port_fixed = true;
          plan.previews.unshift(bp);
          p.ports.push({ port: b.port, label: backendLabel(b) });
          notes.push(`${app.rel} calls ${b.rel} at localhost:${b.port}; it is started as its own preview first`);
        }
      }
      if (!app.fw) notes.push(`Framework not recognised; runs the "${app.scriptName}" script and expects port ${p.port ?? '?'}`);
      plan.notes = notes;
      return { plan, questions };
    }
  }

  const py = analyzePython(root);
  if (py) {
    plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: py.command, port_args: py.args, framework: py.framework, port: py.port, label: 'web', ports: [], env: py.env, ready_path: '/', url_path: '/', needs: [], port_fixed: false, port_from: `${py.name} default` });
    plan.install = py.venv ? null : { command: py.install, dir: root, marker: '.venv', ask: true };
    for (const f of ['manage.py', 'requirements.txt', 'pyproject.toml']) if (exists(path.join(root, f))) relies.push(f);
    plan.framework = py.framework; plan.detected = { kind: 'app', how: `${py.name}${py.venv ? ` (virtualenv ${path.basename(py.venv)})` : ' (no virtualenv yet)'}` };
    plan.notes = notes;
    return { plan, questions };
  }

  const gem = readText(path.join(root, 'Gemfile'));
  if (gem && /['"]rails['"]/.test(gem)) {
    const binDev = exists(path.join(root, 'bin', 'dev'));
    plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: binDev ? 'bin/dev' : 'bin/rails server -b 127.0.0.1', ...(binDev ? { port_env: ['PORT'] } : { port_args: '-p {port}' }), framework: 'rails', port: 3000, label: 'web', ports: [], env: {}, ready_path: '/', url_path: '/', needs: [], port_fixed: false, port_from: 'Rails default' });
    plan.install = { command: 'bundle install', dir: root, check: 'bundle check' };
    relies.push('Gemfile'); plan.framework = 'rails'; plan.detected = { kind: 'app', how: binDev ? 'Rails via bin/dev' : 'Rails server' };
    plan.notes = notes;
    return { plan, questions };
  }

  const orch = findOrchestrator(root, null, 'npm', []);
  if (orch?.kind === 'script') {
    const ports = [...(orch.text || '').matchAll(/(?:localhost|127\.0\.0\.1):(\d{2,5})|(?:--port|-p|PORT=)\s*=?\s*(\d{2,5})\b/g)].map((m) => Number(m[1] || m[2]));
    plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: orch.command, framework: 'custom', port: ports[0] || null, label: 'web', ports: [], env: {}, ready_path: '/', url_path: '/', needs: /docker/.test(orch.text) ? ['docker'] : [], port_fixed: true, port_from: orch.file });
    relies.push(orch.file);
    questions.push(ports[0] ? `Confirm the port: ${orch.file} mentions ${[...new Set(ports)].join(', ')}; ${ports[0]} was chosen.` : `ASK: Which port does \`${orch.command}\` serve the app on? Rerun with --port N.`);
    plan.detected = { kind: 'orchestrator', how: orch.command };
    plan.notes = notes;
    return { plan, questions };
  }

  const compose = parseCompose(root);
  if (compose) {
    const web = compose.services.find((s) => s.ports.length && /web|app|front|site|nginx|caddy/i.test(s.name)) || compose.services.find((s) => s.ports.length);
    if (web) {
      plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: 'docker compose up', framework: 'docker-compose', port: web.ports[0].host, label: 'web', ports: dedupePorts(compose.services.map(browserFacing).filter(Boolean), web.ports[0].host), env: {}, ready_path: '/', url_path: '/', needs: ['docker'], port_fixed: true, port_from: compose.file });
      relies.push(compose.file); plan.detected = { kind: 'compose', how: `docker compose (${compose.file})` };
      questions.push(`Confirm: the app is served by the "${web.name}" service on port ${web.ports[0].host}.`);
      plan.notes = notes;
      return { plan, questions };
    }
  }

  const staticDir = STATIC_DIRS.find((d) => exists(path.join(root, d, 'index.html')));
  if (staticDir) {
    plan.previews.push({ id: idBase, name: idBase, action: 'start', directory: root, command: `node ${q(SELF)} serve${staticDir === '.' ? '' : ` --root ${q(staticDir)}`}`, port_args: '--port {port}', framework: 'static', port: 8080, label: 'web', ports: [], env: {}, ready_path: '/', url_path: '/', needs: [], port_fixed: false, port_from: 'static default' });
    relies.push(path.join(staticDir, 'index.html')); plan.framework = 'static'; plan.detected = { kind: 'static', how: `static files in ${staticDir}` };
    plan.notes = notes;
    return { plan, questions };
  }

  throw new Stop(1, `Could not work out how to run ${tilde(root)}. Read its README (and any Makefile, scripts/ or docs), then run: start --command "<command>" --port <port> [--label web] (the script saves it in the profile).`);
}

function dedupePorts(list, main) {
  const seen = new Set([main]); const out = [];
  for (const p of list) if (p?.port && !seen.has(p.port)) { seen.add(p.port); out.push({ port: p.port, label: p.label || String(p.port) }); }
  return out;
}

function scanLocalhostPorts(dir) {
  const ports = new Set();
  const walk = (d, depth) => {
    if (depth > 4) return;
    for (const n of readdirSafe(d)) {
      if (n === 'node_modules' || n.startsWith('.next') || n === 'dist' || n === 'build' || (n.startsWith('.') && !n.startsWith('.env'))) continue;
      const p = path.join(d, n);
      if (isDir(p)) walk(p, depth + 1);
      else if (/\.(m?[jt]sx?|vue|svelte|astro|json)$|^\.env/.test(n)) {
        const t = readText(p);
        if (t && t.length < 400000) for (const m of t.matchAll(/(?:localhost|127\.0\.0\.1):(\d{2,5})/g)) ports.add(Number(m[1]));
      }
    }
  };
  walk(dir, 0);
  return ports;
}

// ---------------------------------------------------------------------------------------------
// Printing

function printPlan(plan, where) {
  console.log(`Project: ${tilde(plan.project)}${plan.repo ? '' : ' (not a git repository)'}`);
  if (plan.detected?.how) console.log(`Runs with: ${plan.detected.how}`);
  if (plan.detected?.apps?.length) console.log(`Apps it starts: ${plan.detected.apps.join(', ')}`);
  for (const p of plan.previews) {
    const extra = (p.ports || []).map((x) => `${x.port} ${x.label}`).join(', ');
    console.log(`Preview "${p.id}" (${p.action}${p.after ? ` after ${p.after}` : ''}): ${p.command ? `\`${p.port_args ? `${p.command}${/^npm run \S+$/.test(p.command.trim()) ? ' --' : ''} ${p.port_args}` : p.command}\`` : 'register the running server'} in ${tilde(p.directory)}`);
    console.log(`  port ${p.port ?? '?'} ${p.label || 'web'}${p.port_from ? ` (from ${p.port_from})` : ''}${extra ? `; also ${extra}` : ''}${p.needs?.length ? `; needs ${p.needs.join(', ')}` : ''}`);
    if (p.problem) console.log(`  PROBLEM: ${p.problem}`);
  }
  for (const n of plan.notes || []) console.log(`Note: ${n}`);
  if (where) console.log(`${where}`);
}

function logPath(id) { return path.join(LOGS, `${id}.log`); }
function logTail(id, lines = 40) {
  const t = readText(logPath(id));
  if (!t) return '';
  return stripAnsi(t).split('\n').filter((l) => l.trim()).slice(-lines).join('\n');
}

const DIAGNOSES = [
  [/EADDRINUSE|address already in use|Port \d+ is (?:already )?in use|is already in use|Something is already running on port/i,
    'A port is already taken.', 'If it is a leftover of this project: `start --free-ports`. Otherwise choose another port: `start --port <n>` (single apps), or ask the owner to stop the other program.'],
  [/Missing script|ERR_PNPM_NO_SCRIPT|Script not found|error Command "[^"]+" not found|Could not find (?:a )?script|No script found/i,
    'The command runs a package script that does not exist.', 'Pick the right one from package.json "scripts" and run `profile set previews.0.command="<pm> run <script>"`, then `start`.'],
  [/Cannot find module|MODULE_NOT_FOUND|ERR_MODULE_NOT_FOUND|Failed to resolve import|Could not resolve|command not found|: not found|ENOENT.*spawn|Cannot find package|exit status 127|no such file or directory: \S*bin\//i,
    'A program or module is missing: dependencies are probably not installed (or out of date).', 'Run `start --install` (installs with the project\'s package manager), then check the log again.'],
  [/Cannot connect to the Docker daemon|docker daemon|Docker is not running|Is the docker daemon running/i,
    'Docker is not running.', 'Run `start` again: it starts Docker Desktop for profiles that need it (or `profile set previews.0.needs=["docker"]`).'],
  [/No module named ['"]?([\w.]+)/i, 'A Python module is missing in the virtualenv.', 'Install the requirements into the project virtualenv (`start --install`), or add the missing package after asking the owner.'],
  [/Unable to acquire lock|another instance of next dev/i, 'Another `next dev` for this app is already running.', 'Stop the other one: `start --free-ports` frees this project\'s own leftover servers.'],
  [/EMFILE|too many open files/i, 'Too many open files (file watchers).', 'Add `ulimit -n 65536; ` in front of the command with `profile set`, then `start`.'],
  [/ENOSPC/i, 'No space left (disk or file watchers).', 'Tell the owner; clearing build caches (.next, .turbo) may help.'],
  [/Invalid project directory|Couldn't find any `pages` or `app` directory/i, 'The command runs in the wrong directory.', 'Set the preview\'s directory with `profile set previews.0.directory=<abs path>`.'],
  [/permission denied/i, 'A file is not executable or not readable.', 'For a script, run it with `bash <file>` in the command (`profile set`).'],
];

function diagnose(text, expectedPort) {
  const out = [];
  for (const [re, what, fix] of DIAGNOSES) if (re.test(text)) out.push(`DIAGNOSIS: ${what}\nTRY: ${fix}`);
  const seen = [...stripAnsi(text).matchAll(/(?:https?:\/\/)?(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])\s*:\s*(\d{2,5})/g)].map((m) => Number(m[1]));
  const last = seen[seen.length - 1];
  if (expectedPort && last && last !== expectedPort && !out.length) {
    out.push(`DIAGNOSIS: The log shows a server at port ${last}, but the preview waits on ${expectedPort}.\nTRY: \`start --port ${last}\` if that is the app, or fix the port flag in the profile.`);
  }
  return out;
}

// ---------------------------------------------------------------------------------------------
// Starting

function finalCommand(p, port) {
  let cmd = p.command;
  const env = { ...(p.env || {}) };
  if (p.script_port && port !== p.script_port.port) {
    const s = p.script_port.script;
    if (/&&|\|\||;|\||&/.test(s)) throw new Stop(2, `ASK: The dev script fixes port ${p.script_port.port}, which is taken, and the script is too complex to change on the fly (\`${s}\`). Ask the owner to free the port, or set a command with \`profile set\`.`);
    cmd = `${p.script_port.exec} ${s.replace(String(p.script_port.port), String(port))}`;
  }
  if (p.port_args) {
    // npm hands arguments to the script only after `--`.
    const sep = /^npm run \S+$/.test(cmd.trim()) ? ' --' : '';
    cmd = `${cmd}${sep} ${p.port_args.replaceAll('{port}', String(port))}`;
  }
  for (const k of p.port_env || []) env[k] = String(port);
  return { cmd, env };
}

function wrapForLog(cmd, id) {
  fs.mkdirSync(LOGS, { recursive: true });
  const log = logPath(id);
  if (exists(log)) { try { fs.renameSync(log, path.join(LOGS, `${id}.prev.log`)); } catch {} }
  fs.writeFileSync(log, `# ${now()} ${cmd}\n`);
  const target = log.startsWith(`${HOME}/`) && /^[\w./-]+$/.test(log.slice(HOME.length)) ? `"$HOME${log.slice(HOME.length)}"` : q(log);
  const shell = path.basename(process.env.SHELL || 'zsh');
  if (shell === 'fish') return `begin; ${cmd}; end 2>&1 | tee -a ${target}`;
  // tee ignores Ctrl+C so a dev script's own cleanup output (and the cleanup itself, which would
  // die of SIGPIPE) still gets through after Hako interrupts the tab.
  return `(set -o pipefail; { ${cmd} ; } 2>&1 | (trap '' INT; exec tee -a ${target}))`;
}

async function ensureDocker() {
  if (dockerUp()) return;
  const app = ['/Applications/Docker.app', path.join(HOME, 'Applications/Docker.app'), '/Applications/OrbStack.app'].find(exists);
  if (!app) throw new Stop(1, 'The project needs Docker, and Docker is not running (no Docker Desktop or OrbStack app found). Ask the owner to start it.');
  console.log(`Docker is not running; starting ${path.basename(app, '.app')}...`);
  run('open', ['-g', '-a', app]);
  const t0 = Date.now();
  while (Date.now() - t0 < 120000) {
    await sleep(3000);
    if (dockerUp()) { console.log(`Docker is up (${Math.round((Date.now() - t0) / 1000)} s).`); return; }
  }
  throw new Stop(1, 'Docker did not come up within 2 minutes. Ask the owner to check Docker Desktop.');
}

function installDeps(plan, force) {
  const inst = plan.install;
  if (!inst) return false;
  const missing = inst.marker ? !exists(path.join(inst.dir, inst.marker)) : !(inst.check && run('/bin/sh', ['-c', inst.check], { cwd: inst.dir, timeout: 60000 }).ok);
  if (!missing && !force) return false;
  if (inst.ask && !force) {
    throw new Stop(2, `NEEDS_INSTALL: dependencies are not installed (${inst.marker || inst.check} missing). The install would run \`${inst.command}\` in ${tilde(inst.dir)}. Tell the owner, then rerun with --install.`);
  }
  const bin = inst.command.split(' ')[0];
  if (bin !== 'python3' && !which(bin)) throw new Stop(1, `${bin} is not installed, and the project uses it (${inst.command}). Ask the owner to install ${bin}; do not install it globally yourself.`);
  console.log(`Installing dependencies: \`${inst.command}\` in ${tilde(inst.dir)}${missing ? ` (${inst.marker || 'dependencies'} missing)` : ''}...`);
  const t0 = Date.now();
  const r = run('/bin/sh', ['-c', inst.command], { cwd: inst.dir, timeout: 15 * 60000 });
  if (!r.ok) {
    const tail = stripAnsi(`${r.out}\n${r.err}`).split('\n').filter(Boolean).slice(-25).join('\n');
    throw new Stop(1, `Installing dependencies failed (\`${inst.command}\`, exit ${r.status}):\n${tail}`);
  }
  console.log(`Dependencies installed (${Math.round((Date.now() - t0) / 1000)} s).`);
  return true;
}

async function freePorts(ports, root, L) {
  const freed = [];
  for (const port of ports) {
    const h = await portHolder(port, root, L);
    if (h?.kind === 'project') {
      for (const pid of h.pids) { try { process.kill(pid, 'SIGTERM'); freed.push(`${port} (pid ${pid})`); } catch {} }
    }
  }
  if (freed.length) {
    await sleep(1500);
    for (const port of ports) {
      const L2 = listeners();
      const h = await portHolder(port, root, L2);
      if (h?.kind === 'project') for (const pid of h.pids) { try { process.kill(pid, 'SIGKILL'); } catch {} }
    }
    console.log(`Stopped this project's leftover servers on ${freed.join(', ')}.`);
  }
  return freed;
}

function previewSummary(p) {
  const ports = (p.ports || []).map((x) => `${x.port} ${x.label}`).join(', ');
  return `"${p.name}" (id ${p.id}) is ${p.status.toUpperCase()}${p.message ? ` — ${p.message}` : ''}\n  ports: ${ports || p.port}`;
}

function recordFailure(root, isProfile, message) {
  const file = isProfile ? profilePath(root) : draftPath(root);
  const prof = readJson(file);
  if (!prof) return;
  prof.last_failure = { at: now(), message: message.split('\n')[0].slice(0, 300) };
  prof.updated = now();
  writeJsonAtomic(file, prof);
}

async function cmdStart(opt) {
  const t0 = Date.now();
  const { root, repo, hint, picked } = resolveRoot(opt.dir || process.cwd(), opt);
  if (picked) console.log(`Picked ${picked}.`);
  let plan = !opt.fresh && !opt.app && loadProfile(root);
  let isProfile = !!plan;
  if (plan) {
    const missing = (plan.relies_on || []).filter((f) => !exists(path.join(root, f)));
    const missingDirs = plan.previews.filter((p) => !isDir(p.directory)).map((p) => p.directory);
    if (missing.length || missingDirs.length) {
      console.log(`The saved profile no longer fits (${[...missing, ...missingDirs.map(tilde)].join(', ')} missing); detecting again.`);
      const old = plan; plan = null; isProfile = false;
      opt.id = opt.id || old.previews[0]?.id;
    } else {
      const fp = fingerprint(root, plan.relies_on || []);
      if (plan.fingerprint && fp !== plan.fingerprint) console.log(`Note: ${(plan.relies_on || []).join(', ')} changed since the profile was saved; if the start fails, rerun with --fresh.`);
      console.log(`Using the saved profile (${tilde(profilePath(root))}).`);
    }
  }
  if (!plan && !opt.fresh && !opt.app) plan = loadDraft(root);
  if (!plan) {
    const old = loadProfile(root);
    if (old && !opt.id) opt.id = old.previews[0]?.id;
    const { plan: p, questions } = detect(root, repo, hint, opt);
    plan = p;
    if (old) { plan.notes = [...new Set([...(old.notes || []), ...plan.notes])]; plan.created = old.created; }
    writeJsonAtomic(draftPath(root), plan);
    printPlan(plan);
    const asks = questions.filter((x) => x.startsWith('ASK'));
    if (asks.length && !opt.yes) throw new Stop(2, `${asks.join('\n')}\n(The draft is saved; after the owner answers, rerun start with the choice, or with --yes to accept the recommendation.)`);
    for (const x of questions.filter((x) => !x.startsWith('ASK'))) console.log(`Check: ${x}`);
  }
  // One-off choices given on the command line become part of the profile.
  const main = plan.previews.find((p) => p.action === 'start') || plan.previews[0];
  if (opt.command) { main.command = String(opt.command); delete main.port_args; delete main.port_env; delete main.script_port; main.port_fixed = true; main.framework = main.framework || 'custom'; }
  if (opt.port) main.port = Number(opt.port);
  if (opt.label) main.label = String(opt.label);
  if (opt.id && previewId(opt.id) !== main.id) { main.id = previewId(opt.id); main.name = main.id; }
  for (const p of plan.previews) if (p.problem && !p.command) throw new Stop(1, `${p.problem}. Pass --command "<command>" --port <n>.`);
  for (const p of plan.previews) if (!p.port) throw new Stop(2, `ASK: Which port does "${p.id}" serve on? Rerun with --port N.`);

  // Ids must not clash with another project's previews.
  const state = IN_HAKO ? readState() : { previews: [] };
  for (const p of plan.previews) {
    const clash = state.previews.find((x) => x.id === p.id && path.resolve(x.directory) !== path.resolve(p.directory) && !inside(x.directory, root))
      || allProfiles().find((o) => o.project !== root && o.previews?.some((x) => x.id === p.id));
    if (clash) { const nid = `${p.id.slice(0, 35)}-${sha(root).slice(0, 4)}`; for (const o of plan.previews) if (o.after === p.id) o.after = nid; p.id = nid; p.name = nid; }
  }

  if (!IN_HAKO) {
    const lines = plan.previews.filter((p) => p.command).map((p) => { const { cmd, env } = finalCommand(p, p.port); return `  cd ${q(p.directory)} && ${Object.entries(env).map(([k, v]) => `${k}=${q(v)} `).join('')}${cmd}    # port ${p.port}`; });
    throw new Stop(3, `NOT_IN_HAKO: this shell was not started by Hako (HAKO_PREVIEWS is not set), so nothing can appear on the phone. To run it in this terminal instead:\n${lines.join('\n')}\nThen open http://localhost:${main.port}${main.url_path || '/'} on this Mac.`);
  }

  if (plan.previews.some((p) => (p.needs || []).includes('docker'))) await ensureDocker();
  installDeps(plan, !!opt.install);

  const L = listeners();
  const results = [];
  const usedPorts = new Set();
  for (const p of plan.previews) {
    const allPorts = [p.port, ...(p.ports || []).map((x) => x.port)];
    const existing = readState().previews.find((x) => x.id === p.id);

    if (p.action === 'attach') {
      if (existing?.status === 'running' && existing.port === p.port && !opt.restart && (await httpAnswers(p.port))) {
        console.log(`"${p.id}" is already registered and answering on port ${p.port}; reusing it.`);
        results.push({ preview: existing, reused: true });
        continue;
      }
      console.log(`Waiting for "${p.id}" on port ${p.port} (started by ${p.after || 'the project'})...`);
      const ok = await waitForPort(p.port, opt.timeout ? Number(opt.timeout) : 150);
      if (!ok) { console.log(`WARNING: nothing answered on ${p.port}, so "${p.id}" was not registered.`); continue; }
      const res = await attach(p, repo);
      results.push(res);
      continue;
    }

    if (existing && ['running', 'starting'].includes(existing.status) && [p.port, p.last_port].includes(existing.port) && !opt.restart && !(opt['free-ports'] && !existing.command) && (await httpAnswers(p.port, p.ready_path || '/'))) {
      console.log(`"${p.id}" is already running on port ${p.port} (since ${new Date(existing.started).toLocaleTimeString()}); reusing it. (--restart starts it again.)`);
      results.push({ preview: existing, reused: true });
      for (const port of allPorts) usedPorts.add(port);
      continue;
    }

    // Ports.
    let port = p.port;
    const holders = [];
    for (const pt of allPorts) {
      const h = await portHolder(pt, root, L);
      if (h) holders.push({ port: pt, ...h });
    }
    const mainHolder = holders.find((h) => h.port === port);
    const trackedByHako = (pt) => readState().previews.some((x) => x.id !== p.id && ['running', 'starting'].includes(x.status) && (x.ports || []).some((y) => y.port === pt));
    if (opt['free-ports'] && holders.some((h) => h.kind === 'project')) {
      await freePorts(holders.filter((h) => h.kind === 'project').map((h) => h.port), root, L);
      holders.splice(0, holders.length, ...(await Promise.all(allPorts.map(async (pt) => { const h = await portHolder(pt, root, listeners()); return h ? { port: pt, ...h } : null; }))).filter(Boolean));
    }
    const mh = holders.find((h) => h.port === port);
    if (mh && (mh.kind === 'project' || mh.kind === 'project-docker') && !opt['free-ports']) {
      console.log(`Port ${port} is already served by this project (${mh.desc}), started outside Hako; registering it instead of starting a second copy. (To have Hako run it instead: --free-ports.)`);
      const res = await attach({ ...p, action: 'attach' }, repo);
      results.push(res);
      for (const pt of allPorts) usedPorts.add(pt);
      continue;
    }
    if (mh) {
      if (!p.port_fixed || p.script_port) {
        port = await nextFreePort(p.port + 1, L, usedPorts);
        console.log(`Port ${p.port} is taken by ${mh.desc}; using ${port}.`);
      } else {
        throw new Stop(2, `PORTS_BUSY: port ${port} (${p.label}), which this project always uses, is taken by ${mh.desc}${trackedByHako(port) ? ' (another Hako preview)' : ''}. Ask the owner whether to stop that program; this script will not stop processes that do not belong to the project.`);
      }
    }
    const strays = holders.filter((h) => h.port !== p.port && h.kind === 'project');
    if (strays.length) {
      throw new Stop(2, `PORTS_BUSY: ports ${strays.map((h) => h.port).join(', ')} are held by leftovers of this project (${strays.map((h) => h.desc).join('; ')}) while ${p.port} is free, so the dev command would collide with them. They belong to this project: rerun with --free-ports to stop them first.`);
    }
    const foreign = holders.filter((h) => h.port !== p.port && ['other', 'docker'].includes(h.kind));
    for (const h of foreign) console.log(`WARNING: port ${h.port} (${(p.ports.find((x) => x.port === h.port) || {}).label}) is taken by ${h.desc}; the project may fail to start that part.`);

    // Hako sends Ctrl+C to the tab an attached entry inherited, and the idle shell's exit status
    // 130 is then taken for the new command's. Forgetting the attached entry first avoids that.
    const prior = readState().previews.find((x) => x.id === p.id);
    if (prior && !prior.command && prior.tab_id) {
      const rm = sendRequest({ action: 'remove', id: p.id });
      await waitFor(p.id, rm, 15, (x) => !x);
    }
    const { cmd, env } = finalCommand(p, port);
    const request = {
      action: 'start', id: p.id, name: p.name || p.id, directory: p.directory, command: wrapForLog(cmd, p.id),
      port, label: p.label || 'web', ports: (p.ports || []).map((x) => ({ port: x.port, label: x.label })), env,
      url_path: p.url_path || '/', ready_path: p.ready_path || '/', repo: repo || null,
    };
    console.log(`Starting "${p.id}": \`${cmd}\` in ${tilde(p.directory)} (port ${port}${p.ports?.length ? `, also ${p.ports.map((x) => x.port).join(', ')}` : ''}); log: ${tilde(logPath(p.id))}`);
    const req = sendRequest(request);
    const res = await waitFor(p.id, req, Number(opt.timeout || DEFAULT_WAIT_S), (x) => x && ['running', 'failed'].includes(x.status), (x, s) => {
      const last = logTail(p.id, 1);
      console.log(`  ...${x?.status || 'waiting'} (${Math.round(s)} s)${last ? `: ${last.slice(0, 160)}` : ''}`);
    });
    if (res.timeout) {
      if (res.preview?.status === 'starting') throw new Stop(4, `STILL_STARTING: "${p.id}" is still starting after ${Math.round(res.seconds)} s (Hako gives it up to 180 s). Run \`wait --id ${p.id}\` to keep waiting.\nLast log lines:\n${logTail(p.id, 15)}`);
      throw new Stop(1, `"${p.id}" did not report back in time.`);
    }
    if (res.preview.status === 'failed') {
      const tail = logTail(p.id, 40);
      const diag = diagnose(`${res.preview.message || ''}\n${tail}`, port);
      const scripts = readJson(path.join(p.directory, 'package.json'))?.scripts;
      if (scripts && diag.some((d) => d.includes('package script'))) diag.push(`Scripts in ${tilde(path.join(p.directory, 'package.json'))}:\n${Object.entries(scripts).map(([k, v]) => `  ${k}: ${v}`).join('\n')}`);
      const msg = `FAILED: "${p.id}" — ${res.preview.message || 'no message'}\nLast log lines (${tilde(logPath(p.id))}):\n${tail || '(empty log)'}\n${diag.join('\n')}`;
      recordFailure(root, isProfile, res.preview.message || 'failed');
      throw new Stop(1, msg);
    }
    results.push({ preview: res.preview, seconds: res.seconds, port });
    for (const pt of [port, ...(p.ports || []).map((x) => x.port)]) usedPorts.add(pt);
  }

  if (!results.length) throw new Stop(1, 'Nothing was started.');

  // The other ports should answer too once the main one does.
  const extraPorts = results.flatMap((r) => (r.preview.ports || []).filter((x) => x.port !== r.preview.port));
  if (extraPorts.length) {
    const deadline = Date.now() + 30000;
    let pending = extraPorts;
    while (pending.length && Date.now() < deadline) {
      const checks = await Promise.all(pending.map((x) => portOpen(x.port)));
      pending = pending.filter((_, i) => !checks[i]);
      if (pending.length) await sleep(1000);
    }
    if (pending.length) console.log(`WARNING: not answering yet: ${pending.map((x) => `${x.port} ${x.label}`).join(', ')} (it may still be starting; check \`logs\`).`);
  }

  // Save the profile.
  const ports = new Map(results.map((r) => [r.preview.id, r.port]));
  for (const p of plan.previews) if (ports.get(p.id) && !p.port_fixed) p.last_port = ports.get(p.id);
  plan.last_success = now();

  plan.last_start_seconds = Math.round((Date.now() - t0) / 1000);
  plan.fingerprint = fingerprint(root, plan.relies_on || []);
  plan.updated = now();
  writeJsonAtomic(profilePath(root), plan);
  try { fs.unlinkSync(draftPath(root)); } catch {}

  console.log('');
  for (const r of results) console.log(`READY: ${previewSummary(readState().previews.find((x) => x.id === r.preview.id) || r.preview)}${r.reused ? '\n  (already running; no new "Preview ready" notification is sent)' : ''}`);
  console.log(`On the phone: Hako Remote > Previews (opens in the Mac browser). On this Mac: http://localhost:${results[0].preview.port}${results[0].preview.url_path || '/'}`);
  console.log(`Profile saved: ${tilde(profilePath(root))} (${plan.last_start_seconds} s this run).`);
}

async function waitForPort(port, seconds) {
  const t0 = Date.now();
  while (Date.now() - t0 < seconds * 1000) {
    if (await httpAnswers(port, '/', 2000)) return true;
    await sleep(1000);
  }
  return false;
}

async function attach(p, repo) {
  const request = { action: 'attach', id: p.id, name: p.name || p.id, directory: p.directory, port: p.port, label: p.label || 'web', ports: (p.ports || []).map((x) => ({ port: x.port, label: x.label })), url_path: p.url_path || '/', repo: repo || null };
  const req = sendRequest(request);
  const res = await waitFor(p.id, req, 20, (x) => x && ['running', 'failed', 'stopped'].includes(x.status));
  if (res.timeout || res.preview?.status !== 'running') throw new Stop(1, `Registering the running server "${p.id}" on port ${p.port} failed: ${res.preview?.message || 'no answer'}`);
  return { preview: res.preview, seconds: res.seconds, port: p.port, attached: true };
}

// ---------------------------------------------------------------------------------------------
// Other commands

function idsFor(opt) {
  if (opt.id) return [String(opt.id)];
  const { root } = resolveRoot(opt.dir || process.cwd(), opt);
  const prof = loadProfile(root) || loadDraft(root);
  if (!prof) throw new Stop(1, `No profile for ${tilde(root)}; pass --id <preview id> (see \`status --all\`).`);
  return prof.previews.map((p) => p.id);
}

function requireHako() {
  if (!IN_HAKO) throw new Stop(3, 'NOT_IN_HAKO: HAKO_PREVIEWS/HAKO_DATA_DIR are not set; this shell was not started by Hako.');
}

async function cmdControl(action, opt) {
  requireHako();
  const ids = idsFor(opt);
  const order = action === 'restart' ? ids : [...ids].reverse();
  for (const id of order) {
    const known = readState().previews.find((x) => x.id === id);
    if (!known) { console.log(`"${id}" is not in Hako's previews.`); continue; }
    if (action === 'restart' && !known.command) {
      console.log(`"${id}" was registered while running elsewhere; Hako looks for it again.`);
    }
    const req = sendRequest({ action, id });
    if (action === 'remove') {
      const res = await waitFor(id, req, 15, (x) => !x);
      console.log(res.timeout ? `"${id}" is still listed.` : `Removed "${id}".`);
      continue;
    }
    const want = action === 'stop' ? ['stopped', 'failed'] : ['running', 'failed'];
    const res = await waitFor(id, req, action === 'stop' ? 25 : Number(opt.timeout || DEFAULT_WAIT_S), (x) => x && want.includes(x.status));
    if (res.timeout) console.log(`"${id}": still ${res.preview?.status} after ${Math.round(res.seconds)} s.`);
    else console.log(previewSummary(res.preview));
    if (action === 'restart' && res.preview?.status === 'failed') {
      console.log(`Last log lines:\n${logTail(id, 30)}\n${diagnose(logTail(id, 60), res.preview.port).join('\n')}`);
      process.exitCode = 1;
    }
    if (action === 'stop' && !known.command) console.log(`  (Hako only stopped following it; the server was started elsewhere. To stop that server: \`free-port ${known.port}\`.)`);
  }
}

async function cmdWait(opt) {
  requireHako();
  const ids = idsFor(opt);
  for (const id of ids) {
    const t0 = Date.now();
    for (;;) {
      const p = readState().previews.find((x) => x.id === id);
      if (!p) { console.log(`"${id}" is not in Hako's previews.`); break; }
      if (p.status !== 'starting') {
        console.log(previewSummary(p));
        if (p.status === 'failed') { console.log(`Last log lines:\n${logTail(id, 40)}\n${diagnose(`${p.message}\n${logTail(id, 60)}`, p.port).join('\n')}`); process.exitCode = 1; }
        break;
      }
      if (Date.now() - t0 > Number(opt.timeout || DEFAULT_WAIT_S) * 1000) { console.log(`STILL_STARTING: "${id}"`); process.exitCode = 4; break; }
      await sleep(1000);
    }
  }
}

function cmdStatus(opt) {
  if (!IN_HAKO) console.log('Not inside Hako (HAKO_PREVIEWS is not set); showing nothing from Hako.');
  const st = IN_HAKO ? readState() : { previews: [], rejected: [] };
  let ids = null;
  if (!opt.all) { try { ids = idsFor(opt); } catch { ids = null; } }
  const list = st.previews.filter((p) => !ids || ids.includes(p.id));
  if (!list.length) console.log(ids ? `No Hako preview for this project yet (${ids.join(', ')}).` : 'No previews in Hako.');
  for (const p of list) {
    console.log(`${previewSummary(p)}\n  directory: ${tilde(p.directory)}${p.command ? `; log: ${tilde(logPath(p.id))}` : ' (attached)'}`);
    if (p.status === 'failed') console.log(`  last log lines:\n${logTail(p.id, 15).replace(/^/gm, '    ')}`);
  }
  if (opt.all && st.rejected?.length) console.log(`Rejected requests:\n${st.rejected.map((r) => `  ${r.file}: ${r.message}`).join('\n')}`);
}

function cmdLogs(opt) {
  const ids = idsFor(opt);
  for (const id of ids) {
    console.log(`--- ${tilde(logPath(id))}`);
    console.log(logTail(id, Number(opt.lines || 60)) || '(no log: the preview was attached, or never started by this skill)');
  }
}

function setPath(obj, dotted, value) {
  const keys = dotted.split('.');
  let o = obj;
  for (const k of keys.slice(0, -1)) { if (o[k] === undefined) o[k] = /^\d+$/.test(k) ? [] : {}; o = o[k]; }
  const last = keys[keys.length - 1];
  if (value === undefined) delete o[last]; else o[last] = value;
}

function cmdProfile(sub, rest, opt) {
  const { root } = resolveRoot(opt.dir || process.cwd(), opt);
  const file = exists(profilePath(root)) ? profilePath(root) : draftPath(root);
  const prof = readJson(file);
  switch (sub) {
    case 'path': console.log(profilePath(root)); return;
    case 'show':
      if (!prof) { console.log(`No profile for ${tilde(root)} yet (it would be ${tilde(profilePath(root))}).`); return; }
      console.log(`${tilde(file)}${file.endsWith('.draft.json') ? ' (draft: saved as the profile after the first successful start)' : ''}`);
      console.log(JSON.stringify(prof, null, 2));
      return;
    case 'forget': {
      let n = 0;
      for (const f of [profilePath(root), draftPath(root)]) if (exists(f)) { fs.unlinkSync(f); n++; }
      console.log(n ? `Forgot the profile for ${tilde(root)}.` : 'There was no profile.');
      return;
    }
    case 'save': {
      if (!exists(draftPath(root))) { console.log('No draft to save.'); return; }
      const d = readJson(draftPath(root)); d.updated = now(); d.fingerprint = fingerprint(root, d.relies_on || []);
      writeJsonAtomic(profilePath(root), d); fs.unlinkSync(draftPath(root));
      console.log(`Saved ${tilde(profilePath(root))}.`);
      return;
    }
    case 'set': {
      if (!prof) throw new Stop(1, 'No profile or draft yet: run `start` (or `detect`) first.');
      for (const a of rest) {
        const eq = a.indexOf('=');
        if (eq < 0) throw new Stop(1, `Expected key=value, got "${a}"`);
        const key = a.slice(0, eq); const raw = a.slice(eq + 1);
        let val; if (raw === '') val = undefined; else { try { val = JSON.parse(raw); } catch { val = raw; } }
        setPath(prof, key, val);
        console.log(`${key} = ${JSON.stringify(val)}`);
      }
      prof.updated = now();
      writeJsonAtomic(file, prof);
      return;
    }
    case 'note': {
      if (!prof) throw new Stop(1, 'No profile or draft yet.');
      const text = rest.join(' ').trim();
      if (!text) throw new Stop(1, 'Usage: profile note "<text>"');
      prof.notes = [...(prof.notes || []).filter((n) => n !== text), `${now().slice(0, 10)}: ${text}`];
      prof.updated = now(); writeJsonAtomic(file, prof);
      console.log(`Noted in ${tilde(file)}.`);
      return;
    }
    default: throw new Stop(1, 'Usage: profile show|path|save|forget|set key=value…|note "text"');
  }
}

function words(s) { return new Set(String(s).toLowerCase().replace(/[^a-z0-9 ]+/g, ' ').split(/\s+/).filter((w) => w.length > 2)); }
function similarity(a, b) {
  const A = words(a); const B = words(b);
  const inter = [...A].filter((w) => B.has(w)).length;
  return inter / Math.max(1, Math.min(A.size, B.size));
}

function cmdLearn(opt, rest) {
  const situation = String(opt.situation || rest[0] || '').trim().replace(/\s+/g, ' ');
  const fix = String(opt.fix || rest[1] || '').trim().replace(/\s+/g, ' ');
  if (!situation || !fix) throw new Stop(1, 'Usage: learn --situation "<one line>" --fix "<one line>"');
  if (/\/Users\/|\/home\/|~\//.test(situation + fix)) throw new Stop(1, 'Keep learnings general: no local paths (project-specific facts go into the profile with `profile note`).');
  const text = readText(LEARNINGS) || '# Preview skill: learnings\n';
  const lines = text.split('\n');
  const start = lines.findIndex((l) => /^## Entries/.test(l));
  const head = start >= 0 ? lines.slice(0, start + 1) : [...lines, '', '## Entries'];
  const body = start >= 0 ? lines.slice(start + 1) : [];
  const entries = [];
  for (let i = 0; i < body.length; i++) {
    const m = body[i].match(/^- (\d{4}-\d{2}-\d{2}) Situation: (.*)$/);
    if (m) { const f = (body[i + 1] || '').match(/^\s+Fix: (.*)$/); entries.push({ date: m[1], situation: m[2], fix: f ? f[1] : '' }); if (f) i++; }
  }
  const date = now().slice(0, 10);
  const dup = entries.find((e) => similarity(e.situation, situation) >= 0.7);
  if (dup) { dup.date = date; dup.situation = situation; dup.fix = fix; console.log('Updated an existing learning (same situation).'); }
  else { entries.push({ date, situation, fix }); console.log('Added a learning.'); }
  const out = [...head, '', ...entries.flatMap((e) => [`- ${e.date} Situation: ${e.situation}`, `  Fix: ${e.fix}`]), ''];
  fs.writeFileSync(LEARNINGS, out.join('\n').replace(/\n{3,}/g, '\n\n'));
}

async function cmdFreePort(opt, rest) {
  const ports = [...rest, opt.port].filter(Boolean).map(Number);
  if (!ports.length || ports.some((p) => !p)) throw new Stop(1, 'Usage: free-port <port>… [--dir <project>]');
  const { root } = resolveRoot(opt.dir || process.cwd(), opt);
  const L = listeners();
  const mine = [];
  for (const port of ports) {
    const h = await portHolder(port, root, L);
    if (!h) console.log(`Port ${port} is free.`);
    else if (h.kind !== 'project') { console.log(`Port ${port} is held by ${h.desc}, which is not part of ${tilde(root)}. Ask the owner before stopping it.`); process.exitCode = 2; }
    else mine.push(port);
  }
  if (mine.length) await freePorts(mine, root, L);
}

function cmdDetect(opt) {
  const { root, repo, hint, picked } = resolveRoot(opt.dir || process.cwd(), opt);
  if (picked) console.log(`Picked ${picked}.`);
  const prof = loadProfile(root);
  if (prof && !opt.fresh) { console.log(`A profile exists (${tilde(profilePath(root))}); showing it. Use --fresh to detect again.`); printPlan(prof); return; }
  if (prof && !opt.id) opt.id = prof.previews[0]?.id;
  const { plan, questions } = detect(root, repo, hint, opt);
  writeJsonAtomic(draftPath(root), plan);
  printPlan(plan, `Draft saved: ${tilde(draftPath(root))} (\`start\` uses it).`);
  for (const x of questions) console.log(x.startsWith('ASK') ? x : `Check: ${x}`);
  if (questions.some((x) => x.startsWith('ASK'))) process.exitCode = 2;
}

function cmdEnv() {
  console.log(`Inside Hako: ${IN_HAKO ? 'yes' : 'no'}${DATA_DIR ? ` (HAKO_DATA_DIR=${tilde(DATA_DIR)})` : ''}`);
  if (IN_HAKO) {
    const st = readJson(statePath());
    console.log(`state.json: ${st ? `${st.previews.length} preview(s), updated ${new Date(st.updated).toLocaleString()}` : 'not written yet (no preview so far, or a Hako build without previews)'}`);
  }
  console.log(`Profiles: ${tilde(PROFILES)} (${allProfiles().length})\nLogs: ${tilde(LOGS)}\nLearnings: ${tilde(LEARNINGS)}\nNode: ${process.version}`);
}

// A small static file server for plain HTML folders.
function cmdServe(opt) {
  const root = path.resolve(opt.root || '.');
  const port = Number(opt.port || 8080);
  const types = { html: 'text/html; charset=utf-8', css: 'text/css', js: 'text/javascript', mjs: 'text/javascript', json: 'application/json', svg: 'image/svg+xml', png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', ico: 'image/x-icon', woff: 'font/woff', woff2: 'font/woff2', txt: 'text/plain; charset=utf-8', map: 'application/json', mp4: 'video/mp4', pdf: 'application/pdf', wasm: 'application/wasm' };
  const server = http.createServer((req, res) => {
    let rel = decodeURIComponent(new URL(req.url, 'http://x').pathname);
    let file = path.join(root, rel);
    if (!inside(file, root)) { res.writeHead(403).end(); return; }
    if (isDir(file)) file = path.join(file, 'index.html');
    if (!exists(file) && exists(`${file}.html`)) file = `${file}.html`;
    fs.readFile(file, (err, data) => {
      const code = err ? 404 : 200;
      if (err) { const nf = path.join(root, '404.html'); data = readText(nf) ?? `Not found: ${rel}`; file = nf; }
      res.writeHead(code, { 'content-type': types[path.extname(file).slice(1).toLowerCase()] || 'application/octet-stream', 'cache-control': 'no-store' });
      res.end(data);
      console.log(`${new Date().toLocaleTimeString()} ${code} ${req.method} ${req.url}`);
    });
  });
  server.on('error', (e) => { console.error(`serve: ${e.message}`); process.exit(1); });
  server.listen(port, '127.0.0.1', () => console.log(`Serving ${root} at http://localhost:${port}/`));
}

const HELP = `Hako preview skill script.
  start [--dir D] [--app A] [--port N] [--command C] [--id X] [--label L]
        [--install] [--free-ports] [--restart] [--fresh] [--yes] [--timeout S]
                       detect (or use the saved profile), start in Hako, wait, save the profile
  detect [--dir D] [--fresh]   show what would run, save a draft
  status [--all] | wait | logs [--lines N] | stop | restart | remove   [--id X | --dir D]
  profile show|path|save|forget|set key=value…|note "text"   [--dir D]
  learn --situation "…" --fix "…"   add or update a general lesson in LEARNINGS.md
  free-port <port>… [--dir D]  stop this project's own leftover servers on ports
  serve --port N [--root R]    static file server
  env                          where things are
Exit codes: 0 ok, 1 failed (read the DIAGNOSIS/TRY lines), 2 needs a decision (ASK, PORTS_BUSY,
NEEDS_INSTALL), 3 not inside Hako, 4 still starting (run wait).`;

async function main() {
  const [cmd, ...argv] = process.argv.slice(2);
  const { pos, opt } = parseArgs(argv);
  switch (cmd) {
    case 'start': return cmdStart(opt);
    case 'detect': return cmdDetect(opt);
    case 'status': return cmdStatus(opt);
    case 'wait': return cmdWait(opt);
    case 'logs': return cmdLogs(opt);
    case 'stop': case 'restart': case 'remove': return cmdControl(cmd, opt);
    case 'profile': return cmdProfile(pos[0], pos.slice(1), opt);
    case 'learn': return cmdLearn(opt, pos);
    case 'free-port': return cmdFreePort(opt, pos);
    case 'serve': return cmdServe(opt);
    case 'env': return cmdEnv();
    default: console.log(HELP); if (cmd && cmd !== 'help') process.exitCode = 1;
  }
}

main().catch((e) => {
  if (e instanceof Stop) { console.log(e.message); process.exitCode = e.code; }
  else { console.log(`ERROR (a bug in the preview script; work around it and fix it in ${tilde(SELF)}): ${e.stack}`); process.exitCode = 1; }
});
