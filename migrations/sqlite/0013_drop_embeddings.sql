-- 0013_drop_embeddings.sql — remove the RAG/embedding schema.
--
-- Mirrors the Postgres `0012_drop_embeddings.sql`. Attached documents are now
-- inlined into the prompt instead of being chunked, embedded and retrieved.

drop table if exists chunks;
alter table providers drop column embedding_model;
