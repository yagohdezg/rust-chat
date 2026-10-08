-- 0004_agents_providers.sql — per-user agents and model providers.
--
-- Mirrors the Postgres `agents` / `providers` tables (0001_init.sql). UUIDs are
-- BLOBs and JSON is TEXT, matching how `sqlx` encodes `Uuid` and `Json<T>` here.

create table if not exists providers (
    id         blob primary key,
    user_id    blob references users(id) on delete cascade,
    name       text not null,
    kind       text not null,                       -- openai | anthropic | custom
    base_url   text not null,
    api_key    text,                                -- encrypted at rest later
    created_at text not null,
    unique (user_id, name)
);

create table if not exists agents (
    id              blob primary key,
    user_id         blob not null references users(id) on delete cascade,
    name            text not null,
    instructions    text,
    provider_id     blob references providers(id) on delete set null,
    model           text,
    tools           text not null default '[]',
    sandbox_enabled integer not null default 0,
    created_at      text not null,
    updated_at      text not null
);
