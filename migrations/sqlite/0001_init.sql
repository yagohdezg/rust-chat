-- 0001_init.sql — SQLite schema for rust-chat (dev / single-file deployments)
--
-- UUIDs are stored as BLOB (16 bytes), timestamps as RFC3339 TEXT, and JSON as
-- TEXT — matching how `sqlx` encodes `Uuid`, `DateTime<Utc>` and `Json<Value>`
-- for SQLite.

create table if not exists users (
    id            blob primary key,
    email         text not null unique,
    name          text,
    password_hash text,
    role          text not null default 'user',
    created_at    text not null,
    updated_at    text not null
);

create table if not exists conversations (
    id         blob primary key,
    user_id    blob not null references users(id) on delete cascade,
    agent_id   blob,
    title      text not null default 'New chat',
    created_at text not null,
    updated_at text not null
);
create index if not exists conversations_user_idx on conversations (user_id, updated_at desc);

create table if not exists messages (
    id              blob primary key,
    conversation_id blob not null references conversations(id) on delete cascade,
    role            text not null,
    content         text,
    tool_calls      text,
    tool_call_id    text,
    created_at      text not null
);
create index if not exists messages_conversation_idx on messages (conversation_id, created_at);

create table if not exists files (
    id              blob primary key,
    user_id         blob not null references users(id) on delete cascade,
    conversation_id blob references conversations(id) on delete cascade,
    filename        text not null,
    mime            text,
    size_bytes      integer not null default 0,
    storage_path    text not null,
    created_at      text not null
);

-- Embeddings for RAG. `embedding` is a little-endian f32 blob; search does a
-- brute-force cosine scan in Rust (adequate for personal-scale corpora).
create table if not exists chunks (
    id              blob primary key,
    user_id         blob not null,
    conversation_id blob,
    file_id         blob,
    content         text not null,
    embedding       blob not null,
    created_at      text not null
);
create index if not exists chunks_scope_idx on chunks (user_id, conversation_id);
