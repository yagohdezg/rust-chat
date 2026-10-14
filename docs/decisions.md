# Decisions

A lightweight log of product and architecture decisions. Each entry records the
status, the context, the decision, and the consequences. Append new entries;
keep old ones.

## Cross-document search (embeddings / RAG)

- **Status:** Deferred — decision gate. Not acted on until the trigger below is
  demonstrated.
- **Context:** Attached documents are inlined into the prompt, not embedded.
  `FileWorkspace::attachment_context` reads a conversation's attached text-like
  files and prepends them as a system preamble, bounded at 128 KiB per file and
  256 KiB total. Inlining is exact, needs no extra service, and answers
  instantly for the document sizes this product targets, so the RAG/embedding
  stack was deliberately removed. The legacy `embeddings` (Postgres) and
  `chunks` (SQLite) tables are dropped by migrations; there is no vector store,
  chunker or embedder in the tree.
- **Decision:** Do not reintroduce embeddings or a vector store now. **Inlining
  remains the default path.** Revisit only when a real "search across many
  documents" need appears.
- **Trigger condition:** Cross-document search becomes a demonstrated need —
  for example a user must ask questions across many documents at once, or across
  documents too large to inline, and inlining is no longer sufficient on size,
  cost or latency grounds.
- **Intended design if pursued:**
  - A light embedding model, e.g. multi-MiniLM (384-dim), small enough to run
    cheaply rather than a large hosted model.
  - Indexing runs **asynchronously in the background**, not on the request path,
    so a chat turn never blocks on embedding work.
  - The indexer lives **behind a service boundary** — a `ragd`-style service,
    the same shape as `sandboxd` — so the API process stays lean and the indexer
    can be scaled, restarted or replaced independently of `chat-server`.
  - Inlining stays the default; embeddings would be additive for the specific
    cross-document case, not a replacement.
- **Consequences:** No vector store, no chunker, no embedder and no extra
  service are added now. The gate is documented here so the decision is
  explicit; acting on it requires the trigger above.
