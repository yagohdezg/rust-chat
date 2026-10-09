-- 0011_provider_embedding_model.sql — per-provider embedding model (BYOK RAG).
--
-- When set, the provider also serves an OpenAI-compatible `/embeddings`
-- endpoint for that model, using the caller's own key/credential. Embeddings are
-- then billed to the user, never a shared/admin key. Providers without this set
-- simply cannot be used for vector search.

alter table providers add column if not exists embedding_model text;
