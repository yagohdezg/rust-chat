-- 0002_pgvector.sql — optional semantic search / RAG storage
-- Requires the pgvector extension to be available to the Postgres instance.
-- Guarded so the core app still migrates on a vanilla Postgres.

do $$
begin
    if exists (select 1 from pg_available_extensions where name = 'vector') then
        create extension if not exists vector;

        execute $ddl$
            create table if not exists embeddings (
                id              uuid primary key default gen_random_uuid(),
                user_id         uuid not null references users(id) on delete cascade,
                conversation_id uuid references conversations(id) on delete cascade,
                file_id         uuid references files(id) on delete cascade,
                content         text not null,
                embedding       vector(1536) not null,
                created_at      timestamptz not null default now()
            );
            create index if not exists embeddings_ivfflat_idx
                on embeddings using ivfflat (embedding vector_cosine_ops) with (lists = 100);
        $ddl$;
    else
        raise notice 'pgvector not available; skipping embeddings table';
    end if;
end
$$;
