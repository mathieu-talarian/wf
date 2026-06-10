# Activity Feed UI (web) — design

- **Date:** 2026-06-10
- **Status:** Approved design. Deliverable: implementation plan only — **execution is gated on sub-project B** shipping the events read endpoint pinned in §2.
- **Repos:** UI in `../workflow` (React SPA); the API contract in §2 binds the Rust backend (sub-project B).
- **Parent:** A1 spec (`2026-06-08-sub-project-a1-poll-event-backbone-design.md`) — the `events` table this feeds from.

## 1. Purpose & decisions

A new **`/activity` page** showing the unified GitHub + Jira event timeline persisted by the A1 backbone.

Locked during brainstorming:
| Decision | Choice |
|---|---|
| Placement | Own page `/activity` (new TanStack route + nav anchor) |
| v1 features | Source/type filters, repo/project filter, day grouping, infinite scroll — all four |
| Filtering | **Server-side** (approach B): filter params on the endpoint; client-side filtering would produce unevenly-filled pages under infinite scroll |
| Realtime | None in v1 — poll `refetchInterval: 60_000` on the newest page (app convention); SSE arrives with B proper |

## 2. API contract (pins sub-project B's read endpoint)

`GET /api/me/events` — bearer-authed, OpenAPI'd (operationId **`listEvents`**, tag **`events`**) so Orval generates the typed client.

Query params (all optional):
| param | type | meaning |
|---|---|---|
| `before` | i64 | return events with `id < before` (backwards pagination; omit for newest page) |
| `limit` | u32 | page size, default 50, max 100 |
| `source` | string | `github` \| `jira` |
| `typePrefix` | string | SQL prefix match on `type` (e.g. `github.pull_request.`) |
| `scopeKey` | string | exact repo full-name / Jira project key |

Response `200` (camelCase):
```json
{
  "events": [
    { "id": 123, "source": "github", "type": "github.pull_request.merged",
      "scopeKey": "owner/repo", "actor": "octocat", "title": "Add feature",
      "url": "https://github.com/...", "occurredAt": "2026-06-10T12:00:00.000Z",
      "payload": { } }
  ],
  "nextBefore": 74
}
```
- `events` ordered **newest-first** (`ORDER BY id DESC`); `nextBefore` = the last event's `id`, or `null` when the page wasn't full (end reached).
- `?since=<id>` (ascending catch-up, for SSE resume) remains part of B but is **not** consumed by this UI v1.
- Backed by existing indexes: `(user_id, id)` and `(user_id, scope_key, id)`.

## 3. Frontend architecture (`../workflow`)

New files (patterns mirror `dashboard.tsx` / `jira-queue.tsx` / Mantine idioms):

| File | Responsibility |
|---|---|
| `src/routes/activity.tsx` | Route: session guard (`useRedirectIfAnon` pattern), page chrome, mounts the view |
| `src/lib/use-activity-feed.ts` | Hand-written `useInfiniteQuery` over the **Orval-generated `listEvents` fetch function** (no orval-config churn for infinite hooks); filters live in the query key → filter change restarts pagination; `getNextPageParam: (last) => last.nextBefore ?? undefined`; `refetchInterval: 60_000` refreshes page 1 |
| `src/lib/activity.ts` | Pure helpers: day-bucket labelling (Today / Yesterday / `Intl.DateTimeFormat` date), relative time, event-type → family mapping (icon/badge/color) |
| `src/components/activity-filter-bar.tsx` | Source chips (GitHub/Jira), type-family chips (Workflow runs / Pull requests / Issues → `typePrefix`), scope `Select` populated from the **connection-status queries** (selected repos + projects — already fetched app-side), clear-all |
| `src/components/activity-day-group.tsx` | Date header + `Stack` of cards |
| `src/components/activity-event-card.tsx` | Mantine `Card`: family icon, `title` → `url` anchor, actor + relative time, status `Badge` (PR state / run conclusion / Jira status from `payload`) |
| `src/components/activity-feed-view.tsx` | Assembles: filter bar → grouped pages → infinite-scroll sentinel (`IntersectionObserver` triggering `fetchNextPage`); loading / error (problem+json, `DashboardError` pattern) / empty ("events appear after the next sync") states |

Plus: nav `Anchor` for `/activity` in `__root.tsx` `AuthLinks` (between Dashboard and Settings).

**Codegen flow when B lands:** `yarn api:spec && yarn api:gen` → `src/lib/client/endpoints/events/` + models appear; the hook imports from there.

## 4. Behaviour details

- **Day grouping** is computed client-side from `occurredAt` across the flattened pages; groups can span page boundaries (the grouper runs over the concatenated list, not per page).
- **Filters** reset pagination (new query key) and scroll to top. Type-family chips and source chips are mutually composable; selecting a Jira-only family auto-implies `source=jira` (the param sent is just `typePrefix`).
- **Empty payloads/nulls:** `actor`/`title`/`url` are nullable — card falls back to `type`-derived text and renders no anchor when `url` is null.
- **Volume:** no virtualization in v1 (pages of 50, bounded by scroll depth); TanStack Virtual is the documented escape hatch if it ever matters.

## 5. Verification (no test infra exists in this repo — do not introduce one)

- Gates: `yarn type` (tsgo + effect-ls strict) · `yarn lint` · `yarn build`.
- Manual smoke (needs B running in `../wf`): connect, generate activity (merge a PR / transition an issue / run a workflow), tick, verify the feed shows it grouped under Today; exercise each filter; scroll past 50 events (seed or lower `limit`) to verify infinite scroll; kill the network mid-scroll to verify the error state.

## 6. Out of scope (v1)

SSE/live updates; virtualization; per-event detail views beyond the outbound link; persistence of filter state across sessions; the dashboard "recent activity" strip (the "Both" placement option — revisit after v1).
