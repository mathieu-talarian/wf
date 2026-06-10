# Activity Feed UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `/activity` page in the web app (`/Users/mathieumoullec/work/workflow`) showing the unified GitHub+Jira event timeline with source/type/scope filters, day grouping, and infinite scroll (spec: `../wf/docs/superpowers/specs/2026-06-10-activity-feed-ui-design.md`).

**Architecture:** Hand-written `useInfiniteQuery` over the Orval-generated `listEvents` fetch function; filters in the query key; pure helpers for day-bucketing and type-family mapping; Mantine components mirroring the existing dashboard idioms; new TanStack file-route + nav anchor.

**Tech Stack:** React 19, TanStack Router/Query, Mantine, Orval-generated client (fetch + `apiFetch` mutator), TypeScript strict (`tsgo`).

**⛔ PRECONDITION (do not start without it):** sub-project B's `GET /api/me/events` exists in the Rust backend exactly per spec §2 (operationId `listEvents`, tag `events`, params `before/limit/source/typePrefix/scopeKey`, response `{events, nextBefore}`). Task 1 verifies this.

**House rules (this repo):** components are arrow fns typed `(): JSX.Element`; types end in `T`; imports use `@/` alias; NO test framework exists — do not add one; gates are `yarn type`, `yarn lint`, `yarn build`. Run all yarn commands in `/Users/mathieumoullec/work/workflow`. Commit per task with concise messages (this repo's style: short imperative).

---

### Task 1: Regenerate the API client and pin the generated names

**Files:** Modify: `openapi.json`, `src/lib/client/**` (all generated)

- [ ] **Step 1:** `yarn api:spec` (copies `../wf/crates/api/openapi.json`) — then grep `openapi.json` for `"listEvents"`. If absent, **STOP: the precondition isn't met**; report BLOCKED.
- [ ] **Step 2:** `yarn api:gen` — expect `src/lib/client/endpoints/events/` to appear (tags-split). Open it and record the ACTUAL generated names: the fetch function (expected `listEvents`), its params type (expected `ListEventsParams`), and the response model (expected `EventsPage` + `EventDto` in `src/lib/client/models/`). **All later tasks use whatever names Orval actually generated — adjust the code below mechanically if they differ.**
- [ ] **Step 3:** `yarn type && yarn lint` — clean (generated code must not break the build).
- [ ] **Step 4:** Commit: `git add openapi.json src/lib/client && git commit -m "api: regenerate client with listEvents"`

---

### Task 2: Pure helpers (`src/lib/activity.ts`)

**Files:** Create: `src/lib/activity.ts`

- [ ] **Step 1:** Create the file:

```ts
import type { EventDto } from "@/lib/client/models";

// Maps an event `type` to a display family. The families double as the
// filter-bar chips (spec §4): the chip sends `typePrefix` to the server.
export interface EventFamilyT {
  key: "runs" | "prs" | "issues";
  label: string;
  typePrefix: string;
  color: string; // Mantine color token
}

export const EVENT_FAMILIES: readonly EventFamilyT[] = [
  { key: "runs", label: "Workflow runs", typePrefix: "github.workflow_run.", color: "grape" },
  { key: "prs", label: "Pull requests", typePrefix: "github.pull_request.", color: "blue" },
  { key: "issues", label: "Issues", typePrefix: "jira.issue.", color: "teal" },
] as const;

export const familyOf = (type: string): EventFamilyT | undefined =>
  EVENT_FAMILIES.find((f) => type.startsWith(f.typePrefix));

// "Merged" | "Failure" | "In Progress" … — the short badge text per event.
export const badgeOf = (e: EventDto): string | undefined => {
  const payload = e.payload as Record<string, unknown> | null;
  if (e.type.startsWith("github.workflow_run.")) {
    return typeof payload?.conclusion === "string" ? payload.conclusion : undefined;
  }
  if (e.type.startsWith("github.pull_request.")) {
    return e.type.split(".").at(-1);
  }
  if (e.type.startsWith("jira.issue.")) {
    return typeof payload?.statusName === "string" ? payload.statusName : undefined;
  }
  return undefined;
};

const dayFormat = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  month: "short",
  day: "numeric",
});

// Day-bucket label for a timestamp: Today / Yesterday / "Mon, Jun 8".
export const dayLabelOf = (iso: string, now: Date = new Date()): string => {
  const d = new Date(iso);
  const startOf = (x: Date): number =>
    new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diffDays = Math.round((startOf(now) - startOf(d)) / 86_400_000);
  if (diffDays <= 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  return dayFormat.format(d);
};

// Groups a newest-first event list into contiguous day buckets, preserving
// order. Runs over the FLATTENED pages so groups span page boundaries.
export interface DayGroupT {
  label: string;
  events: EventDto[];
}

export const groupByDay = (events: readonly EventDto[]): DayGroupT[] => {
  const groups: DayGroupT[] = [];
  for (const e of events) {
    const label = dayLabelOf(e.occurredAt);
    const last = groups.at(-1);
    if (last && last.label === label) last.events.push(e);
    else groups.push({ label, events: [e] });
  }
  return groups;
};

const RELATIVE_STEPS: readonly [number, string][] = [
  [60, "s"],
  [60, "m"],
  [24, "h"],
];

// "5m ago" / "3h ago" / "2d ago" — coarse relative time for card metadata.
export const relativeTime = (iso: string, now: Date = new Date()): string => {
  let value = Math.max(0, (now.getTime() - new Date(iso).getTime()) / 1000);
  let unit = "s";
  for (const [step, next] of RELATIVE_STEPS) {
    if (value < step) break;
    value /= step;
    unit = next === "s" ? "m" : next === "m" ? "h" : "d";
  }
  return `${Math.floor(value)}${unit} ago`;
};
```

(If `EventDto`'s actual generated name differs, import that name; same for `occurredAt` casing — verify against the generated model from Task 1.)

- [ ] **Step 2:** `yarn type && yarn lint` — clean.
- [ ] **Step 3:** Commit: `git commit -am "activity: pure helpers (families, day buckets, relative time)"`

---

### Task 3: Feed hook (`src/lib/use-activity-feed.ts`)

**Files:** Create: `src/lib/use-activity-feed.ts`

- [ ] **Step 1:**

```ts
import type { EventDto, EventsPage } from "@/lib/client/models";
import type { UseInfiniteQueryResult } from "@tanstack/react-query";

import { useInfiniteQuery } from "@tanstack/react-query";

import { listEvents } from "@/lib/client/endpoints/events/events";

export interface ActivityFiltersT {
  source?: "github" | "jira";
  typePrefix?: string;
  scopeKey?: string;
}

const PAGE_SIZE = 50;

// Infinite, newest-first feed. Filters are part of the query key, so any
// filter change restarts pagination from the newest page (spec §4). The
// 60s refetch refreshes page 1 only (TanStack refetches all loaded pages on
// interval — acceptable at v1 scroll depths; revisit with virtualization).
export const useActivityFeed = (
  enabled: boolean,
  filters: ActivityFiltersT,
): UseInfiniteQueryResult<{ events: EventDto[]; }> & { events: EventDto[] } => {
  const query = useInfiniteQuery({
    queryKey: ["me", "events", filters] as const,
    queryFn: ({ pageParam }) =>
      listEvents({
        limit: PAGE_SIZE,
        ...(pageParam !== undefined ? { before: pageParam } : {}),
        ...(filters.source ? { source: filters.source } : {}),
        ...(filters.typePrefix ? { typePrefix: filters.typePrefix } : {}),
        ...(filters.scopeKey ? { scopeKey: filters.scopeKey } : {}),
      }),
    initialPageParam: undefined as number | undefined,
    getNextPageParam: (last: EventsPage) => last.nextBefore ?? undefined,
    enabled,
    refetchInterval: 60_000,
    staleTime: 30_000,
  });
  const events = query.data?.pages.flatMap((p) => p.events) ?? [];
  return Object.assign(query, { events });
};
```

Adapt to the generated `listEvents` signature from Task 1 (Orval fetch functions take a params object; if it returns `Promise<EventsPage>` directly the above is right — the orval config uses `includeHttpResponseReturnType: false`). If `UseInfiniteQueryResult` typing fights the intersection return, simplify the return type to the query plus a separate `events` value via a tuple or a small interface — keep `yarn type` strict-clean rather than `any`-casting.

- [ ] **Step 2:** `yarn type && yarn lint` — clean.
- [ ] **Step 3:** Commit: `git commit -am "activity: infinite feed hook over listEvents"`

---

### Task 4: Event card (`src/components/activity-event-card.tsx`)

**Files:** Create: `src/components/activity-event-card.tsx`

- [ ] **Step 1:**

```tsx
import type { EventDto } from "@/lib/client/models";
import type { JSX } from "react";

import { Anchor, Badge, Card, Group, Text } from "@mantine/core";
import { FaGithub } from "react-icons/fa";
import { SiJira } from "react-icons/si";

import { badgeOf, familyOf, relativeTime } from "@/lib/activity";

const SourceIcon = ({ source }: { source: string }): JSX.Element =>
  source === "jira" ? <SiJira size={14} /> : <FaGithub size={14} />;

const CardTitle = ({ e }: { e: EventDto }): JSX.Element => {
  const text = e.title ?? e.type;
  return e.url ? (
    <Anchor href={e.url} target="_blank" rel="noreferrer" size="sm" fw={500} lineClamp={1}>
      {text}
    </Anchor>
  ) : (
    <Text size="sm" fw={500} lineClamp={1}>
      {text}
    </Text>
  );
};

export const ActivityEventCard = ({ e }: { e: EventDto }): JSX.Element => {
  const family = familyOf(e.type);
  const badge = badgeOf(e);
  return (
    <Card withBorder radius="md" padding="sm">
      <Group justify="space-between" wrap="nowrap" gap="xs">
        <Group gap="xs" wrap="nowrap" style={{ minWidth: 0 }}>
          <SourceIcon source={e.source} />
          <CardTitle e={e} />
        </Group>
        {badge ? (
          <Badge color={family?.color ?? "gray"} variant="light">
            {badge}
          </Badge>
        ) : null}
      </Group>
      <Group gap="xs" mt={4}>
        <Text size="xs" c="dimmed">
          {e.scopeKey}
        </Text>
        {e.actor ? (
          <Text size="xs" c="dimmed">
            · {e.actor}
          </Text>
        ) : null}
        <Text size="xs" c="dimmed">
          · {relativeTime(e.occurredAt)}
        </Text>
      </Group>
    </Card>
  );
};
```

Check `react-icons/si` exists in deps (the repo already uses `react-icons/fa`); if `SiJira` is unavailable, use `FaTasks` from `react-icons/fa` for Jira instead.

- [ ] **Step 2:** `yarn type && yarn lint` — clean. Commit: `git commit -am "activity: event card"`

---

### Task 5: Day group + filter bar

**Files:** Create: `src/components/activity-day-group.tsx`, `src/components/activity-filter-bar.tsx`

- [ ] **Step 1: `activity-day-group.tsx`:**

```tsx
import type { DayGroupT } from "@/lib/activity";
import type { JSX } from "react";

import { Stack, Text } from "@mantine/core";

import { ActivityEventCard } from "@/components/activity-event-card";

export const ActivityDayGroup = ({ group }: { group: DayGroupT }): JSX.Element => (
  <Stack gap="xs">
    <Text size="xs" c="dimmed" fw={700} tt="uppercase" mt="sm">
      {group.label}
    </Text>
    {group.events.map((e) => (
      <ActivityEventCard key={e.id} e={e} />
    ))}
  </Stack>
);
```

- [ ] **Step 2: `activity-filter-bar.tsx`** — source chips, family chips, scope select. Scope options come from the connection-status data the app already fetches; find the existing hooks/fetchers for GitHub selected repos + Jira selected projects (look in `src/lib/github.ts` / `src/lib/jira.ts` / `src/components/github-repo-select.tsx` for how selected repos are read) and use the same source. Shape:

```tsx
import type { ActivityFiltersT } from "@/lib/use-activity-feed";
import type { JSX } from "react";

import { Button, Chip, Group, Select } from "@mantine/core";

import { EVENT_FAMILIES } from "@/lib/activity";

interface PropsT {
  filters: ActivityFiltersT;
  scopes: string[]; // repo full-names + project keys (from connection status)
  onChange: (next: ActivityFiltersT) => void;
}

const toggled = (current: string | undefined, clicked: string): string | undefined =>
  current === clicked ? undefined : clicked;

export const ActivityFilterBar = ({ filters, scopes, onChange }: PropsT): JSX.Element => (
  <Group gap="xs" wrap="wrap">
    {(["github", "jira"] as const).map((s) => (
      <Chip
        key={s}
        checked={filters.source === s}
        onChange={() => onChange({ ...filters, source: toggled(filters.source, s) as ActivityFiltersT["source"] })}
        size="xs"
      >
        {s === "github" ? "GitHub" : "Jira"}
      </Chip>
    ))}
    {EVENT_FAMILIES.map((f) => (
      <Chip
        key={f.key}
        checked={filters.typePrefix === f.typePrefix}
        onChange={() => onChange({ ...filters, typePrefix: toggled(filters.typePrefix, f.typePrefix) })}
        size="xs"
        color={f.color}
      >
        {f.label}
      </Chip>
    ))}
    <Select
      placeholder="Repo / project"
      data={scopes}
      value={filters.scopeKey ?? null}
      onChange={(v) => onChange({ ...filters, scopeKey: v ?? undefined })}
      clearable
      searchable
      size="xs"
      w={220}
    />
    {filters.source || filters.typePrefix || filters.scopeKey ? (
      <Button variant="subtle" size="compact-xs" onClick={() => onChange({})}>
        Clear
      </Button>
    ) : null}
  </Group>
);
```

- [ ] **Step 3:** `yarn type && yarn lint` — clean. Commit: `git commit -am "activity: day group + filter bar"`

---

### Task 6: Feed view (`src/components/activity-feed-view.tsx`)

**Files:** Create: `src/components/activity-feed-view.tsx`

- [ ] **Step 1:** Assemble filters + grouped list + infinite-scroll sentinel + states. Model the error/empty handling on the dashboard components (read `github-dashboard-view.tsx` for the error-state idiom before writing):

```tsx
import type { JSX } from "react";

import { Alert, Center, Loader, Stack, Text } from "@mantine/core";
import { useEffect, useRef, useState } from "react";

import { ActivityDayGroup } from "@/components/activity-day-group";
import { ActivityFilterBar } from "@/components/activity-filter-bar";
import { groupByDay } from "@/lib/activity";
import { useActivityFeed, type ActivityFiltersT } from "@/lib/use-activity-feed";

// IntersectionObserver sentinel: fires fetchNextPage when scrolled into view.
const useScrollSentinel = (onVisible: () => void): React.RefObject<HTMLDivElement | null> => {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const obs = new IntersectionObserver(([entry]) => {
      if (entry?.isIntersecting) onVisible();
    });
    obs.observe(el);
    return () => obs.disconnect();
  }, [onVisible]);
  return ref;
};

export const ActivityFeedView = ({ enabled, scopes }: { enabled: boolean; scopes: string[] }): JSX.Element => {
  const [filters, setFilters] = useState<ActivityFiltersT>({});
  const feed = useActivityFeed(enabled, filters);
  const sentinel = useScrollSentinel(() => {
    if (feed.hasNextPage && !feed.isFetchingNextPage) void feed.fetchNextPage();
  });

  return (
    <Stack gap="md">
      <ActivityFilterBar filters={filters} scopes={scopes} onChange={setFilters} />
      <FeedBody feed={feed} />
      <div ref={sentinel} />
      {feed.isFetchingNextPage ? (
        <Center>
          <Loader size="sm" />
        </Center>
      ) : null}
    </Stack>
  );
};

const FeedBody = ({ feed }: { feed: ReturnType<typeof useActivityFeed> }): JSX.Element => {
  if (feed.isPending) {
    return (
      <Center mih="30vh">
        <Loader size="sm" />
      </Center>
    );
  }
  if (feed.isError) {
    return (
      <Alert color="red" title="Couldn't load activity">
        {feed.error instanceof Error ? feed.error.message : "Unknown error"}
      </Alert>
    );
  }
  if (feed.events.length === 0) {
    return (
      <Text c="dimmed" size="sm">
        No activity yet — events appear after the next sync.
      </Text>
    );
  }
  return (
    <Stack gap="xs">
      {groupByDay(feed.events).map((g) => (
        <ActivityDayGroup key={g.label} group={g} />
      ))}
    </Stack>
  );
};
```

(Adapt the error rendering to the existing `DashboardError`-style component if it's reusable — prefer reuse over the inline `Alert` if its props fit. `React.RefObject` import shape per the repo's React 19 types.)

- [ ] **Step 2:** `yarn type && yarn lint` — clean. Commit: `git commit -am "activity: feed view with infinite scroll"`

---

### Task 7: Route + nav

**Files:** Create: `src/routes/activity.tsx`; Modify: `src/routes/__root.tsx`

- [ ] **Step 1: `src/routes/activity.tsx`** (session-guard pattern lifted from `dashboard.tsx` — read it first; if `useRedirectIfAnon` is local to dashboard.tsx, replicate it here rather than exporting across routes unless an obvious shared home exists):

```tsx
import type { JSX } from "react";

import { Container, Stack, Title } from "@mantine/core";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";

import { ActivityFeedView } from "@/components/activity-feed-view";
import { useAuth } from "@/lib/auth";

export const Route = createFileRoute("/activity")({
  component: ActivityComponent,
});

const useRedirectIfAnon = (): boolean => {
  const { session, isLoading } = useAuth();
  const navigate = useNavigate();
  useEffect(() => {
    if (!isLoading && !session) {
      void navigate({ to: "/login", replace: true });
    }
  }, [isLoading, session, navigate]);
  return isLoading || !session;
};

function ActivityComponent(): JSX.Element {
  const blocked = useRedirectIfAnon();
  const scopes: string[] = []; // wire from connection-status data per Task 5 Step 2 findings
  return (
    <Container size="md" py="lg">
      <Stack gap="md">
        <Title order={2}>Activity</Title>
        <ActivityFeedView enabled={!blocked} scopes={scopes} />
      </Stack>
    </Container>
  );
}
```

**Note:** Wire `scopes` from the same connection-status source identified in Task 5 (selected repos + selected projects, concatenated); if that data needs a query here, reuse the existing hooks/fetchers rather than new endpoints.

- [ ] **Step 2: nav anchor** in `__root.tsx` `AuthLinks`, between Dashboard and Settings:

```tsx
      <Anchor component={Link} to="/activity" c="dimmed">
        Activity
      </Anchor>
```

- [ ] **Step 3:** `yarn type && yarn lint && yarn build` — clean (the route tree regenerates on build/dev).
- [ ] **Step 4:** Commit: `git commit -am "activity: /activity route + nav"`

---

### Task 8: Manual verification (needs B running)

- [ ] Start the backend (`cargo run -p wf-api` in `../wf`) and `yarn dev`; sign in.
- [ ] Generate provider activity (transition a Jira issue / merge or open a PR / run a workflow), trigger a tick (`curl -X POST localhost:3000/internal/tick -H "X-Internal-Token: …"`), refresh `/activity` — the event appears under **Today** with correct icon/badge/link.
- [ ] Exercise each filter (source, each family chip, a scope) — list updates, pagination restarts.
- [ ] Verify infinite scroll: with >50 events (temporarily lower `PAGE_SIZE` to 5 if needed — revert after), scroll to bottom → next page loads; end of data → sentinel stops fetching.
- [ ] Error state: stop the backend mid-session → red alert renders; restart → recovery on refetch.
- [ ] Empty state: filter to a scope with no events → empty message.
- [ ] Final gates: `yarn type && yarn lint && yarn build`. Commit any fixes.

---

## Plan self-review (done at authoring time)

- **Spec coverage:** §2 contract → Task 1 (verification gate); §3 file map → Tasks 2–7 one-to-one; §4 behaviours → grouping (T2 helper + T6), filter-resets-pagination (T3 query key), null-tolerance (T4 card), family→typePrefix (T2/T5); §5 verification → T8 + per-task gates.
- **Type consistency:** `ActivityFiltersT` defined in T3, consumed T5/T6; `DayGroupT`/`EVENT_FAMILIES`/`groupByDay` defined T2, consumed T5/T6; `EventDto`/`EventsPage` come from generated models (T1 pins actual names and every task defers to them).
- **Known soft spots, marked at point of use:** generated names (T1), `react-icons/si` availability (T4), `DashboardError` reuse (T6), `useRedirectIfAnon` placement and `scopes` wiring (T7) — each has a verify-first instruction and fallback.
