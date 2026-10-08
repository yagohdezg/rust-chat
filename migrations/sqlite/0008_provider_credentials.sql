-- 0008_provider_credentials.sql — per-user API keys for shared providers.
--
-- Mirrors the Postgres `0007_provider_credentials.sql` table. UUIDs are BLOBs
-- and timestamps are RFC3339 TEXT.

create table if not exists provider_credentials (
    user_id     blob not null references users(id) on delete cascade,
    provider_id blob not null references providers(id) on delete cascade,
    api_key     text not null,
    created_at  text not null,
    primary key (user_id, provider_id)
);
