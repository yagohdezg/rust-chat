-- 0012_provider_embedding_model.sql — per-provider embedding model (BYOK RAG).
--
-- Mirrors the Postgres `0011_provider_embedding_model.sql` column. When set,
-- the provider also serves an OpenAI-compatible `/embeddings` endpoint for that
-- model, using the caller's own key; embeddings are billed to the user.

alter table providers add column embedding_model text;
