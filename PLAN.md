# PLAN.md — Missing features and work items

Status snapshot of `rust-chat` and everything that is scaffolded-but-not-wired,
stubbed, or absent. Priorities: **P0** blocks production / correctness,
**P1** required for the product to be usable, **P2** scale/robustness,
**P3** nice-to-have.

Legend: `[ ]` todo, `[~]` partially done, `[x]` done.

---

## 0. Current state (for reference)

Working end-to-end: register/login (JWT + Argon2), conversations, message
history, file upload + UTF-8 RAG indexing, OpenAI-compatible SSE chat,
Podman sandbox exec (direct, in the API process), Postgres + SQLite stores,
`pgvector` and brute-force vector search, chunker. Providers are configured at
runtime (first-run setup, admin global + per-user), agents can be created and
attached to conversations, and `/api/chat` runs the agent/tool loop.

Still scaffolded (not reachable from the server): the MCP client, BoxLite
backend, the standalone `sandboxd` (the API never calls it), and the
`mcp_servers` table.

---

## 1. Agent runtime is dead code — wire it up

`crates/chat-agents` and `crates/chat-mcp` are not dependencies of
`chat-server`, and `/api/chat` (`routes.rs:240`) calls the provider directly
instead of `run_agent`.

- [~] P1 Add `chat-agents` / `chat-mcp` to `chat-server` deps and build a
      `ToolRegistry` + `AgentConfig` in `state.rs` / `main.rs`. `chat-agents` is
      wired (`ToolRegistry` in `AppState`, `AgentConfig` built per request);
      `chat-mcp` is still deferred to §2.
- [x] P1 Route `/api/chat` through `run_agent` when the conversation has an
      agent (or tools) attached; keep the plain path otherwise. Agent path
      triggers on `conversations.agent_id` or a non-empty `ToolRegistry`.
- [x] P1 Stream the agent loop to the client. `run_agent_stream`
      (`chat-agents/src/runtime.rs`) emits `Delta`, `ToolCalls` and `ToolResult`
      events; `/api/chat` forwards them as `delta`, `tool_call`, `tool_result`
      SSE events (the event list rides along on `StreamState` for replay).
- [x] P1 Persist tool calls/results. Tool turns are inserted as separate
      assistant (`tool_calls` JSON) and `tool` messages; history mapping now
      forwards `tool_calls` / `tool_call_id` back to the model. The final answer
      placeholder is moved to the end of the conversation after a tool run.
- [x] P2 Register `CodeInterpreterTool` against the sandbox backend. Opt-in via
      `SANDBOX_TOOL_ENABLED=true` (off by default).
- [x] P2 Per-conversation agent selection: `conversations.agent_id` loads the
      `agents` row (`Store::get_agent`, ownership-checked) to build
      `AgentConfig` — instructions become the system prompt, the agent model
      overrides the request default, and its `tools`/`sandbox_enabled` select
      the tool subset (`ToolRegistry::restricted`). A set `provider_id` is
      resolved via `Store::get_provider` (user or global) into an
      `OpenAiProvider`, falling back to the deployment provider. Agents are
      validated on conversation creation; `Provider` row encryption is handled
      by §3 (`SecretCipher`).

## 2. MCP integration

`chat-mcp` implements a stdio JSON-RPC client but nothing uses it.

- [ ] P2 Add `McpServer` model + `Store` methods for the `mcp_servers` table.
- [ ] P2 Lifecycle: spawn/connect on demand from user config, cache clients,
      reconnect, health checks, shutdown on idle.
- [ ] P2 Adapter that wraps `list_tools` results as `chat_agents::Tool` and
      registers them in the `ToolRegistry`.
- [ ] P2 HTTP/SSE MCP transport (only stdio exists; `transport` column allows
      `http`).
- [ ] P3 Concurrent requests: `StdioMcpClient` is single-flight
      (`chat-mcp/src/lib.rs:3`); add a correlation queue.
- [ ] P3 Sandbox MCP servers (run untrusted MCP servers inside the sandbox).

## 3. Pluggable providers (beyond OpenAI)

Chat providers are now configured at runtime via the database (`providers`),
not the environment; the runtime builds an `OpenAiProvider` per provider row.

- [x] P1 Model discovery: `LlmProvider::list_models()` (OpenAI `/models`) and
      `GET /api/models`; the model is resolved per request (requested/agent
      model, else the provider's first advertised model).

- [x] P1 Provider registry + provider config API: `GET/POST /api/providers`,
      `DELETE /api/providers/{id}`, backed by `Store::{list,create,get,delete}
      _provider`. Chat resolves the provider per turn: agent `provider_id`, else
      the user's first provider, else a global one.
- [x] P1 Admin-provided providers: the first registrant becomes admin; an admin
      creates a global provider (`user_id IS NULL`) that every user inherits,
      and users may add their own on top. A global provider may ship without a
      key: each user stores their own via `PUT /api/providers/{id}/credential`
      (`provider_credentials`, encrypted), and resolution prefers the user's
      credential over the provider key. `GET /api/providers` reports `has_key`
      so the UI prompts for a missing key. Non-admins with no provider are sent
      to the first-run `/setup` page; admins are not forced through it.
- [~] P1 Per-provider model catalog: models are cached in a `models` table keyed
      by `provider_id` (`ProviderModel`, `Store::{list,replace}_provider_models`)
      and served from `GET /api/models` and `GET /api/providers/{id}/models`,
      refreshed lazily after a 1h TTL (falling back to the stale cache if the
      provider errors). Per-scope model allowances are still not tracked and the
      model remains free text.
- [x] P1 Encrypt `providers.api_key` at rest. `SecretCipher`
      (`chat-core/src/crypto.rs`) seals keys with ChaCha20-Poly1305 under a key
      from `SECRET_ENCRYPTION_KEY` (else derived from `JWT_SECRET`); the DB
      backends encrypt on write and decrypt on read, legacy plaintext rows are
      handled transparently and encrypted by a boot/`migrate` backfill.
- [ ] P2 Anthropic-native provider (not just OpenAI-compatible gateways;
      `create_provider` rejects non-`openai`/`custom` kinds).
- [x] P2 Per-request model/provider selection resolved from conversation ->
      agent -> provider (`resolve_agent` in `routes.rs`).
- [~] P2 Admin provider/key management: `PUT /api/providers/{id}` rotates a
      provider's name/base_url/kind/api_key (own providers, or any global one
      for admins; the model cache is invalidated), and
      `POST /api/admin/providers` bulk-creates instance-wide (global) providers
      from a list. A LiteLLM importer (parse `config.yaml` `model_list` or call
      a proxy's `/model/info`) is still pending.
- [ ] P3 Provider failover / retries with backoff on 429/5xx.
- [ ] P3 Token accounting and usage reporting per user.

## 4. Sandbox / execution

### 4a. Missing backends and routing

- [ ] P0 Wire `BoxliteBackend::run` (`chat-sandbox/src/boxlite.rs:39`) against
      the BoxLite OpenAPI exec endpoints; it deliberately fails closed today.
- [ ] P1 Add an `HttpSandboxBackend` so `chat-server` calls `sandboxd` over
      HTTP instead of instantiating a backend in-process (`main.rs:78`). This
      is the whole point of the separate service and is currently not done.
- [ ] P1 Auth between `chat-server` and `sandboxd` (shared token / mTLS);
      `/v1/exec` is currently unauthenticated.
- [ ] P2 gVisor backend (the `IsolationLevel::UserSpaceKernel` variant already
      exists at `chat-sandbox/src/lib.rs:28`) as a cheap, no-KVM middle tier.

### 4b. Per-user "computer" (factory.ai-style) — new subsystem

Current model is ephemeral exec: `podman run --rm` per request. A persistent
per-user computer is a different control plane.

- [ ] P1 Decide persistent-across-sessions vs per-conversation ephemeral
      (drives everything below).
- [ ] P1 `computer-orchestrator` service: create / pause / resume / destroy,
      idle TTL reaper, warm pool.
- [ ] P1 Session placement registry (`user_id -> computer_id -> node`), Redis
      or Postgres. Used for routing and to survive restarts.
- [ ] P1 Sandbox gateway: resolve `computer_id` to a node; avoid sticky LB.
- [ ] P2 Persistent workspace storage (EBS/EFS/S3-backed overlay) and
      snapshot-on-pause. Today `scratch_dir()` is pod-local `/tmp`
      (`chat-sandbox/src/lib.rs:128`) and dies with the run.
- [ ] P2 Warm VM pool to hide Firecracker cold start.
- [ ] P2 Per-computer egress allow-list / proxy.
- [ ] P3 File-transfer API into/out of a computer
      (`ExecRequest.files` only supports inline base64 today).

## 5. Scaling / deployment

### 5a. Correctness blockers at N>1 replicas

- [x] P0 Lift file storage behind a `FileStore` trait with local + S3 impls.
      Trait in `chat-store`; `LocalFileStore` built in, `S3FileStore` in
      `chat-files-s3`. Selected with `FILE_STORAGE_KIND=local|s3`. Download and
      delete routes added.
- [x] P0 Make migrations a one-shot Job/initContainer. `MIGRATE_ON_BOOT`
      (default true) plus a `chat-server migrate` subcommand for a K8s Job.
- [ ] P1 PgBouncer (or equivalent) before scaling pods; each pod opens its own
      `sqlx` pool (`chat-db-postgres/src/lib.rs:31`, `max_connections(20)`).

### 5b. Streaming / networking

- [ ] P1 Document + configure ALB: idle timeout > longest completion, response
      buffering off, connection draining. Every deploy currently cuts in-flight
      SSE streams (`routes.rs:305`).
- [x] P1 Make SSE resumable: assistant placeholder persisted up-front, stream
      checkpointed to the DB, generation detached from the request, and
      `GET /api/messages/{id}/stream` replays/joins via `Last-Event-ID`.
      Live rejoin is per-replica (an in-process `StreamHub`); a shared bus for
      cross-replica rejoin is still pending.
- [ ] P2 Heartbeat/keep-alive tuning (default `KeepAlive` is used; verify it
      beats intermediary idle timeouts).

### 5c. Kubernetes / EKS

- [ ] P2 Manifests: Deployments for `chat-server` + `sandboxd`, Services,
      Ingress, HPA/KEDA.
- [ ] P2 Node groups: `system`, `web`, `sandbox` (bare-metal for `/dev/kvm`,
      tainted). Normal Nitro nodes cannot nest KVM.
- [ ] P2 Autoscale web on CPU/RPS; autoscale sandbox on allocated VM slots /
      queue depth (CPU is the wrong metric for idle VMs).
- [ ] P2 Karpenter + node selectors/taints so web pods never land on metal.
- [ ] P3 Readiness vs liveness split (`/health` is a single liveness probe
      today, `routes.rs:26`). Add `/ready` that checks DB + provider reachability.
- [ ] P3 Graceful shutdown / drain.

## 6. Storage layer

- [ ] P1 Add a transaction around chat turn persistence (insert user message +
      read history + insert assistant message) so partial turns don't persist.
- [x] P1 `delete_file` route + handler, with blob and embedding cleanup via
      `VectorStore::delete_for_file` (`DELETE /api/files/{id}`). Conversation
      delete still pending.
- [~] P1 Vector cleanup on file delete done explicitly. Conversation-delete
      cascade review still open.
- [ ] P2 Conversation management endpoints: rename, delete, archive
      (`conversations` CRUD is create/list/read only).
- [ ] P2 Message pagination (list is capped at 1000, `lib.rs:168`; no cursor).
- [ ] P2 `updated_at` maintenance: bump `conversations.updated_at` on new
      message; add trigger or explicit update.
- [ ] P2 UUIDv7 / monotonic ordering for messages (currently UUIDv4 +
      `created_at`; ordering ties are possible).
- [ ] P2 `sqlite-vec` extension to replace brute-force scan
      (`chat-db-sqlite/src/vector.rs`) for larger corpora.
- [ ] P3 Postgres vector index tuning: HNSW vs ivfflat, reindex/`ANALYZE`
      strategy, tune `lists` (`migrations/postgres/0002_pgvector.sql:21`).
- [ ] P3 Collapse the backends: the SQL lives in two sibling crates
      (`chat-db-postgres`, `chat-db-sqlite`) that each re-implement the same
      `Store`/`VectorStore` surface. Prefer a single `chat-db` crate with
      `postgres` / `sqlite` modules (feature-gated), so a new `Store` method is
      one trait + two impls in one place rather than two crates.

## 7. RAG

- [ ] P1 Configurable embedding dimension. Dimension is hardcoded `1536` in
      `main.rs:58` and in the `vector(1536)` column
      (`migrations/postgres/0002_pgvector.sql:17`); changing
      `EMBEDDING_MODEL` to a different-dim model silently breaks search.
- [ ] P1 Binary document extraction: PDF, DOCX, HTML, Markdown currently
      rejected by `extract_text` (`chat-rag/src/lib.rs:141`). Add a parser
      layer and make extraction async/queued.
- [ ] P2 Async indexing pipeline: indexing currently runs inline on the upload
      request (`routes.rs:197-208`). Move to a job/queue for large files.
- [ ] P2 Re-embedding / re-indexing on model change (version embeddings by
      model).
- [ ] P2 Cross-conversation / global scope retrieval (scope is per-user +
      optional per-conversation).
- [ ] P3 Reranking, hybrid (BM25 + vector) search, score thresholds.
- [ ] P3 Chunk metadata (page/section) surfaced in `sources` SSE events.

## 8. Auth & security

- [x] P1 Refresh tokens / token rotation. `POST /api/auth/refresh` rotates an
      opaque refresh token (stored SHA-256-hashed in `refresh_tokens`), mints a
      new access + refresh pair, and revokes the presented token; replaying a
      revoked token revokes the user's whole session family. `POST
      /api/auth/logout` revokes one. Register/login return both tokens; the web
      client auto-refreshes once on a 401.
- [x] P1 Rate limiting on `/api/auth/*` and `/api/chat`. A process-local
      per-IP fixed-window limiter (`rate_limit.rs`, `RATE_LIMIT_*`) returns 429
      with `Retry-After`; honours `X-Forwarded-For` / `X-Real-IP`.
- [x] P1 Validate `JWT_SECRET` strength at boot: rejects <32 chars and known
      placeholders (`config.rs::validate_jwt_secret`).
- [x] P1 Restrict CORS: `CORS_ALLOWED_ORIGINS` allowlist (localhost dev defaults,
      `*` to opt back into permissive) replaces `CorsLayer::permissive()`.
- [~] P2 RBAC / admin routes: `AdminUser` extractor gates `/api/admin/*`;
      `GET /api/admin/users` lists every account with role, enabled state,
      `last_seen_at` and resource counts; `PATCH /api/admin/users/{id}` changes
      role or enables/disables an account (self-modification is blocked).
      `AuthUser` re-reads the row so a disable or role change takes effect
      immediately, and stale `last_seen_at` is refreshed in the background.
      Still missing: fine-grained per-scope permissions (who may view/use what).
- [ ] P2 API-key auth for programmatic clients.
- [ ] P2 Email verification + password reset flows.
- [x] P2 Audit log for auth and sandbox events. Append-only `audit_logs` table;
      login success/failure, registration, admin user changes and sandbox execs
      are recorded (actor, target, metadata, IP) and surfaced at
      `GET /api/admin/audit` + the `/admin` console.
- [ ] P3 Account lockout / breach-password checks.
- [ ] P3 Upload hardening: max size, MIME sniffing, antivirus, path-traversal
      review (`sanitize_filename` at `routes.rs:375` is a good start).

## 9. Observability

- [ ] P1 Metrics (Prometheus): request rate/latency, SSE stream duration,
      active streams, provider errors/latency, sandbox exec counts, pool
      saturation.
- [ ] P2 Structured JSON logs + request IDs / trace propagation.
- [ ] P2 Distributed tracing spans across chat -> provider -> sandbox.
- [ ] P2 Dashboards + alerts (SLOs: p99 chat TTFT, sandbox failure rate).
- [ ] P3 OpenTelemetry export.

## 10. Testing & CI

- [ ] P1 Integration tests for the HTTP API (only the SQLite store has tests
      today, `chat-db-sqlite/tests/sqlite.rs`). Add a server test harness with
      a fake provider + in-memory store.
- [ ] P1 SSE contract tests (event ordering, terminal `done`, error events).
- [ ] P2 Postgres tests via testcontainers.
- [ ] P2 Sandbox backend contract tests (shared suite across podman/boxlite/
      gVisor/http).
- [ ] P2 Load/soak test for concurrent SSE streams (measures the memory claim
      in the README).
- [ ] P3 Property tests for the chunker (`chat-rag/src/chunker.rs`).
- [ ] P3 CI: build images, run `just ci`, publish on tag.

## 11. Frontend (`web/`)

- [x] P2 Admin console: an `/admin` page (visible only to `role=admin`, linked
      from the sidebar) listing users from `GET /api/admin/users` (role, enabled,
      last seen, resource counts) with role toggle / enable-disable controls,
      global-provider add/delete/key-rotation and bulk import, plus the audit
      log (`GET /api/admin/audit`).
- [ ] P2 Surface agent/tool-call events and `sources` citations in the UI.
- [x] P1 First-run provider setup (`/setup`) and provider list in the sidebar;
      redirect from `/chat` when no provider is configured.
- [x] P1 Agent picker + management: a sidebar "Agents" section (create/list/
      delete), an agent `<select>` in the composer, and an agent chip in the
      header. New chats and existing conversations can be bound to an agent.
- [ ] P2 Move provider management off the left sidebar: the provider list/add/
      delete should live at the bottom-right of the chat (a panel/popover
      anchored near the composer), not as a sidebar section.
- [~] P2 Model picker fed by `GET /api/models`: models populate a `<select>`
      from the default provider (text input only as fallback), but it is not
      grouped by provider and ignores per-agent provider differences.
- [ ] P2 "Thinking" waiting animation: a fun animated inline SVG shown while an
      assistant reply is pending (before the first token / during tool calls).
- [ ] P2 File management (list/download/delete) and upload progress.
- [~] P2 Conversation management UI: delete exists; rename/archive and a proper
      agent-rebind control are still pending (the PATCH endpoint is in place).
- [ ] P2 Per-conversation three-dot menu: add an overflow (⋮) opener beside each
      conversation entry in the sidebar with **rename**, **delete**, and
      **duplicate**. Rename reuses the PATCH endpoint; duplicate needs a new
      `POST /api/conversations/{id}/duplicate` (copy the conversation, its
      messages, and its `agent_id`, with fresh ids).
- [ ] P2 Streaming reconnect/resume handling.
- [ ] P3 Sandbox "computer" view (persistent workspace, file browser).

## 12. Documentation / housekeeping

- [ ] P1 Update README to reflect that `chat-server` calls `sandboxd` over
      HTTP once 4a lands (today it does not).
- [ ] P2 Document the target EKS topology and scaling model.
- [ ] P2 `.env.example`: add any new config (sandboxd URL/token, S3, queue).
- [ ] P3 Remove or label dead code paths until they're wired.

---

## Suggested first milestones

1. **M1 — Correctness at scale:** DONE. S3 `FileStore` (5a), migration Job
   (5a), resumable SSE (5b), file delete + vector cleanup (6).
2. **M2 — Agents live:** wire `run_agent` into `/api/chat` with streaming and
   tool-call persistence (1), register `CodeInterpreterTool` (1).
3. **M3 — Sandbox split:** `HttpSandboxBackend` + `sandboxd` auth (4a), wire
   BoxLite (4a).
4. **M4 — Computer orchestration:** registry + orchestrator + gateway +
   persistent workspaces (4b).
5. **M5 — EKS:** manifests, node groups, autoscaling (5c), observability (9).
