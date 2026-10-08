-- 0005_model_catalog.sql — cached per-provider model catalog.
--
-- Mirrors the Postgres `models` table. UUIDs are BLOBs and timestamps are
-- RFC3339 TEXT, matching how `sqlx` encodes `Uuid` and `DateTime<Utc>` here.

create table if not exists models (
    provider_id blob not null references providers(id) on delete cascade,
    id          text not null,
    owned_by    text,
    fetched_at  text not null,
    primary key (provider_id, id)
);