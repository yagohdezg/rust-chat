-- 0001_init.sql — core relational schema
-- Postgres 16

create extension if not exists "pgcrypto";

create table if not exists users (
    id            uuid primary key default gen_random_uuid(),
    email         text not null unique,
    name          text,
    password_hash text,
    role          text not null default 'user',   -- user | admin
    created_at    timestamptz not null default now(),
    updated_at    timestamptz not null default now()
);

create table if not exists providers (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid references users(id) on delete cascade,
    name       text not null,
    kind       text not null,                       -- openai | anthropic | custom
    base_url   text not null,
    api_key    text,                                -- encrypted at rest later
    created_at timestamptz not null default now(),
    unique (user_id, name)
);

create table if not exists agents (
    id             uuid primary key default gen_random_uuid(),
    user_id        uuid not null references users(id) on delete cascade,
    name           text not null,
    instructions   text,
    provider_id    uuid references providers(id) on delete set null,
    model          text,
    tools          jsonb not null default '[]'::jsonb,
    sandbox_enabled boolean not null default false,
    created_at     timestamptz not null default now(),
    updated_at     timestamptz not null default now()
);

create table if not exists mcp_servers (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid references users(id) on delete cascade,
    name       text not null,
    transport  text not null,                       -- stdio | http
    command    text,                                -- for stdio
    url        text,                                -- for http
    enabled    boolean not null default true,
    created_at timestamptz not null default now()
);

create table if not exists conversations (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid not null references users(id) on delete cascade,
    agent_id   uuid references agents(id) on delete set null,
    title      text not null default 'New chat',
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);
create index if not exists conversations_user_idx on conversations (user_id, updated_at desc);

create table if not exists messages (
    id              uuid primary key default gen_random_uuid(),
    conversation_id uuid not null references conversations(id) on delete cascade,
    role            text not null,                  -- system | user | assistant | tool
    content         text,
    tool_calls      jsonb,                          -- assistant tool_calls
    tool_call_id    text,                           -- tool result linkage
    created_at      timestamptz not null default now()
);
create index if not exists messages_conversation_idx on messages (conversation_id, created_at);

create table if not exists files (
    id              uuid primary key default gen_random_uuid(),
    user_id         uuid not null references users(id) on delete cascade,
    conversation_id uuid references conversations(id) on delete cascade,
    filename        text not null,
    mime            text,
    size_bytes      bigint not null default 0,
    storage_path    text not null,
    created_at      timestamptz not null default now()
);
