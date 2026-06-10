# Workflow — what the app does (functional guide)

This guide describes the server **from the user's point of view**: what each feature is for, what every endpoint lets you do, what you send, and what you get back. No internals — for the technical side see [`ARCHITECTURE.md`](ARCHITECTURE.md); for exact JSON schemas see `GET /api/openapi.json`.

**Workflow in one sentence:** a personal developer dashboard that connects to *your* GitHub and Jira accounts and gives you one place to see everything that needs your attention — pull requests to review, issues assigned to you, CI to run — and to act on it without opening GitHub or Jira.

Everything is **per-user**: you connect your own tokens, you only ever see your own data, and the server acts on GitHub/Jira *as you*.

---

## 1. How requests work (applies everywhere)

- **Who you are.** You sign in through Supabase in the web app. Every personal endpoint (everything under `/api/me/...`) needs your login token in the `Authorization: Bearer <token>` header. No token, or an expired one → `401`.
- **What errors look like.** Every failure is a small JSON problem document: an HTTP status, a short stable error code (`slug`), a human title, a `detail` message, and often a `reason` telling you *why* (e.g. the GitHub token lacks a scope). The UI can show these directly.
- **Format.** All responses are JSON, camelCase fields, timestamps like `2026-06-10T06:00:00.000Z`.
- **Your tokens are safe.** GitHub and Jira tokens are encrypted at rest and are **never returned** by any endpoint — status endpoints only ever show the last 4 characters.

---

## 2. Signing in & your profile

### `GET /api/me`
"Who am I?" Called when the app loads. Confirms your login token is valid and returns your profile (id, email, name, avatar). First call ever also **registers you** in the app's database — there is no separate sign-up step.

---

## 3. Connecting GitHub

To use the GitHub side, you give the app a **personal access token** (classic or fine-grained), then choose which repositories you care about. Everything GitHub-related works only on those selected repos.

### `GET /api/me/github` — "Am I connected?"
The settings-screen status. Returns whether a GitHub account is connected and, if so: the GitHub login, token kind (classic/fine-grained), its scopes, the repos you selected, the token's health (`validationStatus` + error message if it stopped working), when it expires, when it was last checked and last used, and its last 4 characters so you can recognize which token it is.

### `POST /api/me/github/token` — connect (or replace) your token
You paste a token (`{ "token": "ghp_..." }`). The server first checks it against GitHub (does it work? who is it? what can it do?), then stores it encrypted. Replaces any previously stored token. Returns the same status summary as above. A bad token is rejected with the reason.

### `POST /api/me/github/token/validate` — "Is my token still good?"
No input. Re-checks the stored token against GitHub right now and updates its health status — useful when the dashboard starts failing and you want to know if the token expired or was revoked.

### `DELETE /api/me/github` — disconnect
Removes your GitHub connection (and the stored token) entirely. Returns `{ "disconnected": true }`.

### `GET /api/me/github/repos` / `PUT /api/me/github/repos` — choose your repos
- **GET** lists the repositories your token can see, together with which ones are currently selected — the repo-picker screen.
- **PUT** saves your selection (`{ "repos": ["acme/api", "acme/web"] }`), throws away the now-stale dashboard cache, and returns the refreshed connection summary. This selection is the lens for the whole GitHub experience: the dashboard, branch prompts, workflows, and background activity tracking all cover exactly these repos.

---

## 4. The pull-request dashboard

The home screen for GitHub work: "what needs me?"

### `GET /api/me/github/dashboard?tab=`
One call returns your whole PR overview:
- **account** — whose GitHub account this is,
- **queues** — a count per tab so the UI can show badges,
- **queuePulls** — the list of PRs in the currently requested tab (`tab` defaults to `assigned`).

The five tabs:

| Tab (`tab=` / queue key) | What's in it |
|---|---|
| `assigned` | PRs assigned to you |
| `review_requested` | PRs waiting for your review |
| `authored` | PRs you opened |
| `mentioned` | PRs where someone mentioned you |
| `failing_ci` | Your PRs whose checks are red |

**Freshness:** this endpoint answers instantly with the last known picture (even right after a server restart) and refreshes from GitHub in the background — so the dashboard never makes you wait, at the cost of being possibly a minute or two behind. Opening it also nudges the background activity tracker (§10) to refresh you first.

### `GET /api/me/github/queue?key=`
Just one tab's PR list (same keys as above), without the rest of the dashboard — what the UI calls when you switch tabs.

### `GET /api/me/github/pull?owner=&repo=&number=`
Everything about one PR for the detail view, focused on "is this ready?": draft state, changed files, head/base branches, approvals and requested reviewers, the latest CI run and required checks, mergeability, and what's blocking it.

### `POST /api/me/github/pulls/enrich`
The dashboard lists are intentionally light. When the UI needs the expensive extras (review status, CI status, mergeability) for the PRs on screen, it sends the list of PR references (`{ "refs": [{owner, repo, number}, ...] }`) and gets the enriched details back in one batch, one result per requested PR.

---

## 5. From branch to pull request

### `GET /api/me/github/branches`
"You pushed a branch — want to open a PR?" Scans your selected repos for branches that were recently pushed but **have no open PR yet** (branches whose PR was already merged are excluded), and returns them as ready-made prompts (repo, branch, recent commit info, a compare link). Repos it couldn't scan are reported alongside, per repo, instead of failing the whole response. The UI turns each prompt into a one-click "Create PR" card.

### `GET /api/me/github/repo/branches?owner=&repo=`
Lists the branches of one repo — used to fill the base/head pickers in the "create PR" form.

### `POST /api/me/github/pulls` — create a pull request
You send `{ owner, repo, base, head, title, body? }` and the PR is created on GitHub as you. Returns the new PR (number, URL, ...), so the UI can jump straight to it.

---

## 6. Acting on pull requests

### `POST /api/me/github/pull/merge`
Merges a PR: `{ owner, repo, number, method }`, where `method` is `merge`, `squash`, or `rebase`. Returns whether it merged plus GitHub's message; if GitHub refuses (conflicts, branch protection, required reviews), you get the reason back.

### `POST /api/me/github/pull/close`
Closes a PR without merging: `{ owner, repo, number }`.

---

## 7. Running CI workflows

The Actions side: see your workflows, run them with the right inputs, watch the runs.

### `GET /api/me/github/workflows`
All GitHub Actions workflows across your selected repos, grouped per repo (id, name, file path, state, link) — the catalog the UI shows. Repos that couldn't be read report their error per repo without breaking the rest.

### `GET /api/me/github/workflow/inputs?owner=&repo=&path=`
Before running a workflow, the UI asks: "what does this workflow need?" The server reads the workflow file and returns its manual-run (`workflow_dispatch`) inputs — each input's name, type (string/boolean/choice/environment), whether it's required, its default, and the choice options — enough to render the run form dynamically.

### `GET /api/me/github/repo/environments?owner=&repo=`
The repo's deployment environments (staging, production, ...) — for workflows whose input is an environment picker.

### `POST /api/me/github/workflow/dispatch` — run it
`{ owner, repo, workflowId, ref, inputs: { name: value, ... } }` — triggers the workflow on that branch/tag/commit with those inputs, exactly like pressing "Run workflow" on GitHub.

### `GET /api/me/github/workflow/runs?owner=&repo=&workflowId=&branch=`
Recent runs of one workflow on one branch — status, conclusion, timing, link — so you can watch whether the run you just dispatched went green.

### `GET /api/me/github/favorites` / `PUT /api/me/github/favorites`
Your pinned workflows, so the ones you actually use sit at the top. **GET** returns the favorites as a map of repo → workflow ids. **PUT** replaces one repo's list (`{ "repoFullName": "acme/api", "workflowIds": [123, 456] }`) — duplicates are dropped, an empty list un-pins the repo entirely — and returns the full updated map.

---

## 8. Connecting Jira

Same idea as GitHub: you connect with your own Jira Cloud credentials, then choose which projects you care about.

### `GET /api/me/jira` — "Am I connected?"
Whether Jira is connected and, if so: your site, who you are on it (account, email, display name), the projects you selected, credential health, and the token's last 4 characters.

### `POST /api/me/jira/token` — connect
You provide `{ "siteUrl": "https://yourco.atlassian.net", "email": "you@co.com", "token": "<API token>" }`. The server normalizes the site URL, verifies the credentials against Jira (who are you on this site?), and stores the token encrypted. Bad site, email, or token → rejected with the reason.

### `POST /api/me/jira/token/validate` — re-check credentials
Re-verifies the stored credentials right now and updates their health status.

### `DELETE /api/me/jira` — disconnect
Removes the Jira connection. Returns `{ "disconnected": true }`.

### `GET /api/me/jira/projects` / `PUT /api/me/jira/projects` — choose your projects
- **GET** lists the projects you can see on your site.
- **PUT** saves your selection (`{ "projects": ["ENG", "OPS"] }`) — the lens for your Jira dashboard and background activity tracking.

---

## 9. Working with Jira issues

### `GET /api/me/jira/dashboard`
Your whole Jira overview in one call: a count per queue plus the first page of issues, covering five queues:

| Queue key | What's in it |
|---|---|
| `assigned` | Issues currently assigned to you |
| `previously_mine` | Issues that used to be assigned to you (things you handed off but may still care about) |
| `reported` | Issues you created |
| `watching` | Issues you're watching |
| `active_sprint` | Issues in your active sprint |

### `GET /api/me/jira/queue?key=&cursor=`
One queue's issues, **paged**: pass the queue key, get a page of issues plus a `cursor`; pass the cursor back to get the next page. How the UI does infinite scroll per tab.

### `POST /api/me/jira/search`
Power-user search: `{ "jql": "project = ENG AND status = 'In Review'", "cursor"? }` runs any JQL query you write and returns the same paged issue list as a queue.

### `GET /api/me/jira/issue?key=`
The full detail of one issue (e.g. `ENG-123`): summary, description (rich text converted to a renderable form), status, assignee, reporter, priority, labels, dates, comments — the issue detail panel.

### Browsing helpers (for pickers and navigation)

| Endpoint | What it answers |
|---|---|
| `GET /api/me/jira/issuetypes?projectKey=` | "What can I create in this project?" — Bug, Task, Story... |
| `GET /api/me/jira/boards` | Your agile boards. |
| `GET /api/me/jira/sprint/issues?boardId=` | What's in this board's **active sprint** right now. |
| `GET /api/me/jira/issue/transitions?key=` | "Where can this issue go from here?" — the legal next statuses, used to render the status-change menu. |
| `GET /api/me/jira/users?query=&issueKey=&projectKeyOrId=&actionDescriptorId=` | Type-ahead search for people who can be **assigned** in this context — feeds the assignee picker. |
| `GET /api/me/jira/createmeta?projectKey=&issueTypeId=` | "Which fields does the create form need?" — required/optional fields and their allowed values for that project + issue type. The create form is built from this. |
| `GET /api/me/jira/editmeta?key=` | Same, but for editing an existing issue: which of its fields are editable and how. |

---

## 10. Acting on Jira issues

All of these act on Jira **as you**, immediately.

| Endpoint | What you do | What you send |
|---|---|---|
| `POST /api/me/jira/issue/transition` | Move an issue to another status (e.g. → In Progress) | `{ key, transitionId }` (an id from the transitions list) |
| `POST /api/me/jira/issue/comment` | Comment on an issue | `{ key, body }` (plain text; converted to Jira's rich-text format). Returns the created comment. |
| `POST /api/me/jira/issue/assign` | Assign — or **unassign** by sending no account | `{ key, accountId }` (`accountId: null` clears the assignee) |
| `POST /api/me/jira/issue/worklog` | Log time spent | `{ key, timeSpent: "2h 30m", started?, comment? }` (omit `started` → "now") |
| `POST /api/me/jira/issue` | Create a new issue | `{ projectKey, issueTypeId, fields }` — fields are checked against what the create-form metadata (§9) actually allows. Returns the new issue's id and key. |
| `PUT /api/me/jira/issue` | Edit an existing issue's fields | `{ key, fields: { summary: ..., priority: ..., ... } }` |

Writes return either the created object (comment, issue) or a simple `{ "ok": true }`.

---

## 11. Your activity feed

Once you're connected, the server **keeps watching for you** even when the app is closed:

- Every ~2 minutes, a scheduled job wakes the server (`POST /internal/tick` — internal, secret-protected, not callable by users).
- For each connected user it checks their selected GitHub repos (PR activity, workflow runs) and Jira projects (issue changes) for anything new since last time.
- New activity is recorded as **events** in your personal history: "PR #42 was opened", "CI failed on main", "ENG-123 moved to Done by Alice", with who/what/when and a link.
- Visiting your GitHub dashboard bumps you to the front of the line, so active users get the freshest data.
- It's resilient and polite: failing sources are retried with increasing delays, the work is budgeted so the job never runs away, and nothing is ever recorded twice.

The web app shows this history on its **`/activity` page**: a scrollable feed grouped by day (Today / Yesterday / ...), with filter chips and infinite scroll, refreshed every minute. It is built entirely on the endpoint below.

### `GET /api/me/events` — read your history
Your recorded events, **newest first**, paged. All query parameters are optional:

| Parameter | What it does |
|---|---|
| `limit` | Page size, 1–100 (default 50). |
| `before` | Cursor for the next page — pass the `nextBefore` from the previous response. |
| `source` | Only one integration: `github` or `jira`. |
| `typePrefix` | Only event types starting with this prefix, e.g. `github.pull_request.` or `jira.issue.` (matched literally — no wildcards). |
| `scopeKey` | Only one repo (`owner/name`) or Jira project key. |

Returns `{ "events": [...], "nextBefore": ... }`. Each event has: `id`, `source`, `type` (e.g. `github.workflow_run.completed`, `github.pull_request.opened`/`merged`/`closed`, `jira.issue.created`/`transitioned`), `scopeKey`, `actor` (who did it), `title`, `url` (link to the PR/run/issue), `occurredAt`, and a type-specific `payload` (e.g. a run's `conclusion`, an issue's `statusName`). `nextBefore` is `null` when you've reached the end of your history.

**Where events come from and what they cover:** exactly your selected repos and projects (§3, §8). Change the selection and the tracker follows. Events are kept per-user — you only ever see activity from your own scopes.

---

## 12. Service endpoints (not user-facing)

| Endpoint | Purpose |
|---|---|
| `GET /api/health` | "Is the API alive?" — returns `{ status: "ok", time }`. Public. |
| `GET /api/hello/{name}` | Connectivity echo (`{ greeting: "Hello, <name>" }`). Public. |
| `GET /api/openapi.json` | The machine-readable contract of every endpoint above — used to generate the web app's API client and usable in Swagger UI. Public. |
| `GET /healthz` | Infrastructure liveness probe (Cloud Run). Plain `ok`. |
| `POST /internal/tick` | The activity-feed tracker trigger (§11). Requires the internal secret header; returns a run summary (how many sources checked, how many events written). |

---

## 13. The product surface in numbers

- **49 user-facing operations**: 2 system, 1 profile, 22 GitHub, 23 Jira, 1 activity feed — plus 2 infrastructure endpoints.
- **2 integrations** (GitHub, Jira), each with: connect → validate → select scope → dashboard → detail → act.
- **5 PR queues** and **5 issue queues** make up the two dashboards.
- **6 GitHub write actions** (dispatch, create/merge/close PR, repos & favorites selection) and **6 Jira write actions** (transition, comment, assign, worklog, create, edit).
- **1 personal activity feed** (`/activity`), fed by the background tracker across both integrations.
