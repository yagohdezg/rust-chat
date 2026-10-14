-- 0020_api_keys.sql — hashed, scoped API keys for programmatic clients.
--
-- Mirrors the Postgres `0019_api_keys.sql` columns. UUIDs are BLOBs, timestamps
-- are RFC3339 TEXT and JSON is TEXT, matching how `sqlx` encodes `Uuid`,
-- `DateTime<Utc>` and `Json<Value>` here.

create table if not exists api_keys (
    id           blob primary key,
    user_id      blob not null references users(id) on delete cascade,
    name         text not null,
    prefix       text not null,
    key_hash     text not null unique,
    scopes       text not null default '[]',
    created_at   text not null,
    last_used_at text,
    expires_at   text,
    revoked_at   text
);
create index if not exists api_keys_user_idx on api_keys (user_id);
