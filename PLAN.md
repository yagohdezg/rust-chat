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
`pgvector` and brute-force vector search, chunker.

Scaffolded but **not reachable from the server**: agents/tools runtime,
MCP client, BoxLite backend, standalone `sandboxd` (the API never calls it),
`providers`/`agents`/`mcp_servers` tables, `users`/agent config, custom
providers.

---

## 1. Agent runtime is dead code — wire it up

`crates/chat-agents` and `crates/chat-mcp` are not dependencies of
`chat-server`, and `/api/chat` (`routes.rs:240`) calls the provider directly
instead of `run_agent`.

- [ ] P1 Add `chat-agents` / `chat-mcp` to `chat-server` deps and build a
      `ToolRegistry` + `AgentConfig` in `state.rs` / `main.rs`.
- [ ] P1 Route `/api/chat` through `run_agent` when the conversation has an
      agent (or tools) attached; keep the plain path otherwise.
- [ ] P1 Stream the agent loop to the client. `run_agent`
      (`chat-agents/src/runtime.rs:67`) currently buffers the whole turn and
      returns `Vec<ChatMessage>`; it needs a streaming interface that emits
      `delta`, `tool_call`, `tool_result` SSE events as they happen.
- [ ] P1 Persist tool calls/results. `/api/chat` inserts messages with
      `tool_calls=None, tool_call_id=None` (`routes.rs:252,328`); tool turns
      would be lost. Add `ChatMessage` -> `insert_message` mapping.
- [ ] P2 Register `CodeInterpreterTool` against the sandbox backend.
- [ ] P2 Per-conversation agent selection (`conversations.agent_id` exists but
      is unused by the chat path).

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

`providers` table and `kind` column exist; runtime only ever uses
`OpenAiProvider` (`main.rs:73`).

- [ ] P1 Provider registry + per-user provider config API (CRUD over
      `providers`).
- [ ] P1 Encrypt `providers.api_key` at rest (currently plaintext; schema
      comment says "encrypted at rest later").
- [ ] P2 Anthropic-native provider (not just OpenAI-compatible gateways).
- [ ] P2 Per-request model/provider selection resolved from conversation ->
      agent -> provider.
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

- [ ] P1 Refresh tokens / token rotation (access token only today,
      `chat-auth/src/lib.rs:39`).
- [ ] P1 Rate limiting on `/api/auth/*` and `/api/chat` (no throttling today).
- [ ] P1 Validate `JWT_SECRET` strength at boot (accepts `change-me` literally,
      `config.rs:114`).
- [ ] P1 Restrict CORS: `CorsLayer::permissive()` (`main.rs:115`) allows any
      origin even though auth is bearer-token based.
- [ ] P2 RBAC / admin routes (`role` is stored and in JWT but never enforced).
- [ ] P2 API-key auth for programmatic clients.
- [ ] P2 Email verification + password reset flows.
- [ ] P2 Audit log for auth and sandbox events.
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

- [ ] P2 Surface agent/tool-call events and `sources` citations in the UI.
- [ ] P2 File management (list/download/delete) and upload progress.
- [ ] P2 Conversation rename/delete UI.
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
