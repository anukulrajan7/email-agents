# 04 — JavaScript & React Explained

**Summary.** The web app in `apps/web` is a React 19 single-page app with one `fetch` wrapper, TanStack Query for every server read and write, react-router for pages, and Tailwind v4 tokens for a "paper and ink" look. It has 49 Vitest files (291 tests, all passing on 14 Oct 2026) and 30 Playwright specs. It is well organised by feature and consistent in its hooks convention. Its real weaknesses are: no error boundary, server JSON is trusted without validation, polling is everywhere, admin pages are hidden rather than guarded, and a few components have grown past 300 lines. This document explains how it works using the actual files, then lists what to learn and how to prove you learned it.

Paths are relative to `apps/web/src/` unless shown otherwise. Line numbers were correct on 14 Oct 2026 at commit `0e368c8`.

---

## 1. The shape of the app

```mermaid
flowchart TD
  main[main.tsx: fonts, index.css, createRoot] --> App[app/App.tsx]
  App --> Providers[app/providers.tsx: QueryClient + ToastProvider]
  Providers --> Router[BrowserRouter]
  Router --> Routes[app/routes.tsx: Routes/Route table]
  Routes --> RequireAuth[features/auth/RequireAuth.tsx]
  RequireAuth --> Shell[app/AppShell.tsx: nav, Suspense, Outlet, floating widgets]
  Shell --> Page[features/<area>/<Page>.tsx]
  Page --> Hooks[features/<area>/hooks.ts: useQuery / useMutation]
  Hooks --> Client[api/client.ts: api<T>()]
  Client --> API[(Rust API /api/v1)]
```

**A. What we built.** One entry (`main.tsx`), one provider stack, one declarative route table, and one folder per product area under `features/`. Each feature folder has a page, components, a `hooks.ts` that owns all server calls, a `types.ts` where the feature has its own shapes, and co-located `*.test.tsx` files.

**B. Why.** The app has 31 routes and about 26,000 lines of TypeScript. Without a rule like "all fetching lives in `hooks.ts`", 25 features would each invent their own loading, error and cache behaviour. The rule is followed everywhere except `features/auth/ResetPassword.tsx` (two inline mutations) and `features/sources/hooks.ts` (one raw `api` call inside a mutation).

**C. How it works internally.** React renders the tree top down. `Providers` creates the `QueryClient` once with `useState(createQueryClient)` (a lazy initialiser, so the factory runs on the first render only). `BrowserRouter` listens to the History API. `AppRoutes` matches the URL to a `<Route>` and renders its `element`. Nested routes render into the parent's `<Outlet/>`.

**D. Why this approach.** Declarative `<Routes>` is the simplest react-router mode. The app does not use the data-router (`createBrowserRouter` with loaders), because all data goes through TanStack Query anyway. Two systems for loading would be worse than one.

**E. Alternatives.** Next.js app router (server components, file routing) is heavier and needs a Node server; this app is served as static files by the Rust API (`http/mod.rs:215`, `ServeDir` with `index.html` fallback). React-router's data router would give route-level `errorElement` for free; that is the one thing we are missing.

**F. Mistakes.** See §9.

---

## 2. Routing, step by step

File: `app/routes.tsx`.

```tsx
<Route element={<RequireAuth />}>
  <Route path="/w/:ws" element={<AppShell />}>
    <Route path="inbox/:messageId" element={<InboxPage />} />
```

1. The URL `/w/ws_1/inbox/msg_9` is matched segment by segment. `:ws` and `:messageId` become params.
2. `RequireAuth` (6 lines) renders `<Outlet/>` if `localStorage` has a token, else `<Navigate to="/sign-in" replace/>`. It does not check the token is valid; the first API 401 does that (`api/client.ts:74`), clears the token and hard-reloads to `/sign-in`.
3. `AppShell` reads `useWorkspace()` (`app/useWorkspace.ts`): `ws` from `useParams`, the workspace object from the `/me` query. If the user is not a member, it shows "Workspace not found". Admin pages are not guarded here; they are only hidden from the nav (`app/nav.ts`). The server enforces roles (`auth::require_role` in Rust), so this is a UX gap, not a security hole.
4. `<Suspense fallback={<RouteFallback/>}><Outlet/></Suspense>` renders the page. Six pages are code-split with `lazy()` (`routes.tsx:30-35`); the other 25 are in the main bundle.
5. `InboxPage` is mounted for both `inbox` and `inbox/:messageId`. It reads `messageId` and shows a two-pane layout; on mobile it hides the list when a message is open (`InboxPage.tsx:19-26`).

**Concept to name: the URL is state.** The selected message, the workspace and the sort live in the URL or localStorage, not in a store. That is why there is no Redux or Zustand. Question for you: what breaks if two tabs are open and one signs out? (Answer: nothing syncs; the other tab keeps working until its next 401. There is no `storage` event listener.)

---

## 3. The API client

File: `api/client.ts` (83 lines). Read it fully; it is the one place the browser talks to the server.

- `api<T>(path, opts)` builds the URL under `/api/v1`, adds `Accept` and `X-Auth-Token`, JSON-encodes the body unless it is `FormData`, and calls `fetch`.
- Non-2xx: `parseError` reads the server's `{ error: { code, message, request_id } }` shape (the Rust `AppError::into_response` in `apps/api/src/error.rs:85`) into an `ApiError` class with `status`, `code` and `requestId`.
- 401 outside `/auth/*`: clear the token and `window.location.assign('/sign-in')`.
- 204 returns `undefined`. Otherwise `JSON.parse(text) as T`.

**The cast is the gap.** `as T` tells TypeScript to trust the server. Nothing checks the JSON at runtime. If the Rust DTO in `http/dto.rs` renames a field, the TypeScript type in `api/types.ts` still compiles and the UI shows `undefined`. There is no zod, valibot or generated client; `api/types.ts` (510 lines, 97 types) is hand-written and 13 feature folders have their own `types.ts`.

**Concept: the trust boundary.** TypeScript types exist at compile time only. Anything that crosses a network, a file or `localStorage` is `unknown` until you check it. The repo already has one correct example: `isErrorBody` in `client.ts:24` is a type guard that narrows `unknown` to `ApiErrorBody`. Question: why is a type guard enough for the error body but you would want a schema library for `MessageDetail`? (Size and nesting; a guard for 40 fields is unreadable.)

**Why the header is `X-Auth-Token` and not `Authorization`.** The comment at `client.ts:62` explains: a reverse-proxy basic-auth gate once owned `Authorization`. The Rust side accepts both (`auth::request_token`, `apps/api/src/auth.rs:175`). This is a small example of a decision that only makes sense with history; the memory note says the Caddy gate was removed on 13 Oct 2026, so the reason no longer applies but the code is harmless.

---

## 4. Data fetching with TanStack Query

### 4.1 The defaults (`app/providers.tsx`)

- `staleTime: 2000` — data is "fresh" for 2 s; a second component mounting in that window reuses the cache without a request.
- `refetchOnWindowFocus: false`.
- `retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2` — never retry 4xx, retry 5xx and network errors twice.
- Mutations have no defaults: they never retry. Correct, because a retried POST could send an email twice.

### 4.2 The query-key convention

Every workspace-scoped key starts `['ws', ws, …]`. Example factories in `features/inbox/hooks.ts:6-7`:

```ts
export const messagesKey = (ws) => ['ws', ws, 'messages'] as const;
export const messageKey  = (ws, id) => ['ws', ws, 'message', id] as const;
```

Because keys are arrays, `invalidateQueries({ queryKey: ['ws', ws] })` matches everything under a workspace by prefix. `features/assistant/hooks.ts` uses exactly that after a confirmed plan (a broad but safe refetch).

**Concept: cache keys are a hash of inputs.** The key is the identity of the data. `[...messagesKey(ws), filters]` (`inbox/hooks.ts:15`) includes the filter object, so changing the search text creates a new cache entry rather than overwriting the old list. Question: `filters` is an object; why does React Query not break on reference changes? (It hashes keys structurally, not by reference.)

### 4.3 Three real patterns

**Infinite list plus polling** — `useMessages` (`inbox/hooks.ts:13`): `useInfiniteQuery`, `getNextPageParam: last.next_cursor`, `refetchInterval: 5000`. Note the cost: every tick refetches every loaded page. Load three pages and each 5 s poll is three requests.

**Adaptive polling** — `useMessage` (`inbox/hooks.ts:26`): `refetchInterval` is a function of the last result. It polls at 3 s only while `processing_status` is `pending` or `normalized`, or while the parent asked for fast polling (a draft is being generated). This is the right pattern; copy it instead of fixed intervals.

**Mutation chain with cross-feature invalidation** — `useSendDraft` (`features/drafts/hooks.ts`): PATCH the draft if edited, then POST `/drafts/:id/send`, then invalidate `messageKey` and `messagesKey` imported from `inbox/hooks`. The draft feature reaches into the inbox feature's keys. That coupling is acceptable because the keys are exported constants, but it shows why a shared `keys.ts` would be cleaner.

### 4.4 Polling inventory

| Hook | Interval |
|---|---|
| `useMessages` | 5 s, all loaded pages |
| `useMessage` | 3 s while processing or awaiting a draft |
| matching (3 queries) | 10 s |
| workflows, sources (when `poll`) | 5 s |
| reviews, automations | 10 s |
| agents-hub registry, agent outgoing | 15 s |
| agents cases, messages badge | 30 s |
| alerts badge, team metrics | 60 s |
| designer session | 1.5 s while a dry run runs |

`AppShell` mounts four always-on pollers (alerts, approvals, messages, performance widget). On the inbox page with one message open that is roughly 7 requests every 5 s per tab. It works at today's scale and SQLite reads are cheap, but it is the first thing to replace with Server-Sent Events when users notice battery or the server notices load. See `07`.

---

## 5. A full request, traced: generating a draft

User clicks **Generate** in the draft panel.

| Step | File and function | Responsibility | Data | On failure |
|---|---|---|---|---|
| 1 | `features/drafts/DraftPanel.tsx` button `onClick` | UI intent | `{ quality?: 'premium' }` | Button `loading` state |
| 2 | `features/drafts/hooks.ts` `useGenerateDraft` | `useMutation` wrapping the call | same | `ErrorNote` shows `error.message` |
| 3 | `api/client.ts` `api()` | HTTP, auth header, error shape | `POST /api/v1/workspaces/{ws}/messages/{id}/draft` | 401 → redirect; else `ApiError` thrown |
| 4 | `apps/api/src/http/mod.rs:143` route | Match method + path | path params `ws`, `id` | 404 JSON via fallback |
| 5 | `apps/api/src/http/messages.rs:629` `request_draft` | Handler: role check, ownership check, enqueue job | `AuthUser` extractor, `Path`, raw `Bytes` body | `AppError` → JSON `{error:{code,…}}` |
| 6 | `apps/api/src/jobs/mod.rs:92` `enqueue` inside `BEGIN IMMEDIATE` plus `audit::record` | Durable job + audit row in one transaction | `generate_draft` job with a unique `dedupe_key` | SQLite busy → waits up to 15 s |
| 7 | `state.jobs_notify.notify_one()` | Wake the worker without polling | — | — |
| 8 | Response `202 { status: 'queued' }` | Tell the client it is asynchronous | — | — |
| 9 | `DraftPanel` sets `awaitingDraft`, `MessageDetailView` passes `pollFast` into `useMessage` | Poll at 3 s for up to 60 s until `draft.updated_at` changes | `MessageDetail` | After 60 s stops polling, shows nothing new |
| 10 | Worker: `jobs/handlers.rs:127` `generate_draft` → `compose_draft` → `Drafter::draft` (OpenRouter) → write `drafts` row | AI call and persistence | prompt → JSON draft | Retry with backoff; dead job marks the run failed |

**Why 202 and polling, not a long request?** The AI call takes 3 to 20 s and can be retried five times. Holding an HTTP request open that long ties up a connection and fails on proxy timeouts. The job table makes the work durable across restarts.

**Weakness.** Step 9 is a 60 s polling window with no completion signal. Server-Sent Events or a `job_id` to poll would be cleaner. Also `understand_now` (`messages.rs:671`) does the opposite: it runs an AI call inside the HTTP request so the insight card never waits. Two different strategies for similar work is a smell worth fixing with one rule: "model calls that must feel instant run inline with a 10 s cap; everything else is a job".

---

## 6. Rendering behaviour you should understand from this code

**Keys reset state.** `MessageDetailView` is rendered with `key={messageId}` (`InboxPage.tsx:37`), and `DraftEditor` with `key={`${draft.id}:${draft.updated_at}`}`. A new key unmounts the old component and mounts a fresh one, which wipes `useState`. That is how the editor picks up a server-side update without an effect. Question: what would happen if the key were only `draft.id`?

**Effects are for synchronising with things outside React.** Good uses here: the auto-growing textarea (`DraftPanel.tsx:67`), keyboard shortcuts, timers. Questionable uses: `DraftPanel.tsx:319` pushes child state up to the parent through an effect plus callback; `automations/Builder.tsx:115` has an effect with no dependency array, so it runs after every render. `AgentsHubPage.tsx:45` mutates `let index = 0` during render to compute animation delays; under StrictMode's double render that counter double-counts.

**Context for cross-cutting UI only.** Two contexts exist: `ToastProvider` (`components/ui/Toast.tsx`) and `GuideProvider`. `useToast()` returns a no-op when there is no provider, which is why component tests do not need to wrap it. Note the `useCallback` and `useMemo` in `Toast.tsx:20-29`: they keep the `notify` function identity stable so consumers do not re-render on every toast.

**Lazy initialiser.** `useState(createQueryClient)` passes the function, not its result. Passing `createQueryClient()` would build a new client every render and throw it away.

**Closures and stale values.** `UndoBar` in `DraftPanel.tsx` counts down with `setInterval` and reads `sendAt`. Any timer that reads state must either be re-created when that state changes (dependency array) or read from a ref. Exercise 3 below makes you break and fix this.

---

## 7. Styling and the design system

`index.css` uses Tailwind v4's CSS-first `@theme`. `--color-*: initial` wipes the default palette, so only project tokens exist: `paper`, `surface`, `raised`, `ink`, `ink-muted`, `rule`, `accent`, and status tints. Fonts are Fraunces (display), Hanken Grotesk (body), JetBrains Mono. Animations live under `@media (prefers-reduced-motion: no-preference)`. There is no `tailwind.config.*` and no design-system document; `components/ui/index.ts` is the de facto component catalogue (Button, Input, Select, Dialog, Tabs, Table, Skeleton, Toast, EmptyState, and so on).

Memory note from the owner: UI must stay "minimal but designed", Gmail-like for email bodies, and never show model names. The `no-model-names.spec.ts` e2e test enforces the last rule.

---

## 8. Tests

- **Vitest** (`vite.config.ts` `test` block): jsdom, `globals`, setup imports jest-dom matchers. 49 files, 291 tests, 15.7 s. API calls are faked with `vi.stubGlobal('fetch', …)` and URL matching; there is no MSW. Shared fixtures in `test/fixtures.ts`.
- **Playwright** (`e2e/`): chromium only, one worker, a real Rust API started with `AI_MOCK=1` and a mock Nango/Gmail (`e2e/support/mock-nango.mjs`). Global setup signs up three users and seeds a workspace. 99 real tests and 63 `test.fixme` placeholders. The agent builder, agents hub, approvals and autonomy flows have no browser coverage yet.
- **Gaps:** no test for `app/routes.tsx`; `features/jobs` and `features/reviews` have no tests; `api/client.test.ts` has a test named "adds the bearer token" that actually asserts `X-Auth-Token`.

---

## 9. Mistakes and weaknesses (frontend)

Severity follows `07-engineering-mistakes-and-improvements.md`. CONFIRMED means the code was read; HYPOTHESIS needs a test.

| # | Severity | Issue | Where | Why it matters | Fix | How to test |
|---|---|---|---|---|---|---|
| F1 | High | No error boundary anywhere; a render error or a failed lazy chunk blanks the whole app | `app/App.tsx`, `app/AppShell.tsx` | One bad field from the API kills every page | One `ErrorBoundary` around `<Outlet/>` with a "Reload" button; a second around `AskMeera` | Throw inside a page in a Vitest render; assert the fallback renders and the nav survives |
| F2 | High | Server JSON is cast, never validated | `api/client.ts:82`, `api/types.ts` | Type drift between Rust DTOs and TS is silent | Generate types from the Rust DTOs (`schemars` + `openapi-typescript`), or add zod schemas for the five biggest responses and parse at the boundary | A test that feeds a wrong-shaped `MessageDetail` and expects a thrown `ApiError('schema')` |
| F3 | Medium | Polling everywhere; `useMessages` refetches every loaded page every 5 s | `inbox/hooks.ts:22` and §4.4 | Load and battery; grows with "Load more" | Keep adaptive polling; replace the list poll and badges with one SSE stream of `{kind, id}` events that invalidates keys | Count requests in a Playwright trace over 60 s before and after |
| F4 | Medium | Admin routes hidden, not guarded | `app/routes.tsx`, `app/nav.ts` | Operators can reach `/w/:ws/jobs` and see a server 403 as a raw error | A `RequireAdmin` layout route using `useWorkspace().isAdmin` | Playwright: operator visits `/jobs`, sees an explanation, not an error |
| F5 | Medium | Components over 300 lines with 7 to 10 `useState`s | `AssistantPanel.tsx:46-361`, `SignInPage.tsx:195-418`, `IntegrationsPage.tsx`, `DraftPanel.tsx` | Hard to test, hard to change | Split by responsibility (form state into `useReducer`, voice into its own component) | Existing tests must still pass unchanged |
| F6 | Medium | 25 of 31 routes in the main bundle; assistant and voice code eagerly loaded | `app/routes.tsx:30`, `AppShell.tsx` | First load pays for every page | `lazy()` every page; lazy-mount `AssistantPanel` on first open | `pnpm build` output: main chunk size before and after |
| F7 | Low | Index keys on derived lists | `DraftPanel.tsx:84`, `PlainTextBody.tsx:58`, `AttachmentChips.tsx:38` | Fine for append-free lists; a habit that bites later | Key by a stable id or `${start}-${text}` | — |
| F8 | Low | Effect without dependency array | `automations/Builder.tsx:115` | Runs after every render | Add deps or move focus logic into the event handler | — |
| F9 | Low | Uncleaned timeouts | `Toast.tsx:25`, `DraftPanel.tsx:205`, `IntegrationsPage.tsx:143` | setState after unmount warnings | Return a cleanup from the effect, or keep ids in a ref | — |
| F10 | Low | Dead code and stale names | `agent-builder/hooks.ts:37` `useEditSpec`; `api/client.test.ts` test name | Confusion | Delete; rename | — |
| F11 | Low | No skip link, static `<title>`, no focus move on route change | `AppShell.tsx`, `index.html` | Screen-reader users land in the nav every time | `document.title` per route; a skip link; focus `<main>` on navigation | axe e2e plus manual tab test |
| F12 | Low | `ws` prop-drilled through 3 to 4 levels although `useWorkspace()` exists | `MessageDetailView` → `DraftPanel` → `DraftEditor` → `UndoBar` | Noise | Read `ws` from the hook at the leaf | — |

---

## 10. What to learn, in order

1. **Promises and async control flow** — `api()` is `async`, mutations return promises, `useSendDraft` chains two awaits. Read MDN "Using promises" and write Exercise 1.
2. **The trust boundary and type narrowing** — `isErrorBody` type guard; MDN/TS handbook "Narrowing". Exercise 2.
3. **Closures and timers** — `UndoBar` countdown. Exercise 3.
4. **React rendering: keys, state, effects** — react.dev "You Might Not Need an Effect" and "Preserving and Resetting State". Exercise 4.
5. **Server state with TanStack Query** — docs: "Important Defaults", "Query Keys", "Invalidation from Mutations", "Infinite Queries". Exercise 5.
6. **Error boundaries and Suspense** — react.dev "Catching rendering errors with an Error Boundary". Exercise 6.
7. **Testing** — Testing Library "Guiding principles"; Vitest `vi.stubGlobal`. Exercise 7.

Official docs: [react.dev](https://react.dev/learn) · [TanStack Query v5](https://tanstack.com/query/v5/docs/framework/react/overview) · [React Router](https://reactrouter.com/) · [TypeScript handbook: Narrowing](https://www.typescriptlang.org/docs/handbook/2/narrowing.html) · [Vite](https://vite.dev/guide/) · [Tailwind v4 theme](https://tailwindcss.com/docs/theme) · [Vitest](https://vitest.dev/guide/) · [Playwright](https://playwright.dev/docs/intro).

---

## 11. Exercises (do them in order; each is under an hour)

1. **Promises.** Without running it, write down the order of console output if `api()` throws inside `useSendDraft`'s first `await`. Then add a `console.log` and check. Question: where does the error end up and who renders it?
2. **Narrowing.** Write `isMessageSummary(value: unknown): value is MessageSummary` checking only `id`, `subject`, `from`. Wire it into one hook with `.then(v => { if (!isMessageSummary(v)) throw new ApiError(500, 'schema', 'bad shape'); return v; })`. Write one Vitest test that feeds a wrong shape.
3. **Closures.** In `UndoBar`, remove `sendAt` from the effect dependency array, generate a draft, send it, then regenerate while the countdown is running. Predict first, then observe, then restore. Explain the stale closure in two sentences.
4. **Keys.** Change `key={messageId}` on `MessageDetailView` to a constant. Open two different messages in a row. Predict which state leaks (hint: `awaitingDraft`). Restore it.
5. **Query invalidation.** Add a new mutation `useArchiveMessage` in `inbox/hooks.ts` that calls an endpoint you stub, and make the list update without a manual refresh. Then make it optimistic with `onMutate` and `setQueryData`, and roll back on error.
6. **Error boundary.** Add `components/ErrorBoundary.tsx` (class component, `getDerivedStateFromError`), wrap `<Outlet/>` in `AppShell`, and write a test that throws inside a child and asserts the fallback text.
7. **Testing.** Write the missing test for `features/jobs/JobsPage.tsx`: stub `fetch` to return two dead jobs, click Retry, assert the POST path.

**Three understanding questions** (answer in writing before moving on):

- Why does a 401 do `window.location.assign` instead of `navigate('/sign-in')`, and what is the cost of that choice?
- Why does the draft panel key the editor by `updated_at` rather than storing the body in a `useEffect`?
- If the Rust API renamed `next_cursor` to `cursor`, which line fails first, and what would the user see?

---

## 12. Next steps

- Ship F1 (error boundary) and F4 (admin guard) first; both are under 40 lines each.
- Decide on F2: generated types versus zod at the boundary. Recommendation: `schemars` on the Rust DTOs and `openapi-typescript`, so the contract has one source.
- Replace list polling with one SSE endpoint after the backend work in `07` Phase 2.
