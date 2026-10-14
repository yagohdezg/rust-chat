-- 0019_api_keys.sql — hashed, scoped API keys for programmatic clients.
--
-- Only a SHA-256 hash of each key is stored; `prefix` keeps the visible head of
-- the key so it can be identified in a list. `scopes` narrows the key to a
-- subset of the owner's role permissions. Rows cascade away with their user.

create table if not exists api_keys (
    id           uuid primary key default gen_random_uuid(),
    user_id      uuid not null references users(id) on delete cascade,
    name         text not null,
    prefix       text not null,
    key_hash     text not null unique,
    scopes       jsonb not null default '[]'::jsonb,
    created_at   timestamptz not null default now(),
    last_used_at timestamptz,
    expires_at   timestamptz,
    revoked_at   timestamptz
);
create index if not exists api_keys_user_idx on api_keys (user_id);
