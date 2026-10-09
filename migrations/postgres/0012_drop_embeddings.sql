-- 0012_drop_embeddings.sql — remove the RAG/embedding schema.
--
-- Attached documents are now inlined into the prompt instead of being chunked,
-- embedded and retrieved, so the vector store and the per-provider embedding
-- model are gone. The pgvector extension is left in place (harmless).

drop table if exists embeddings;
alter table providers drop column if exists embedding_model;
