-- 0004_model_catalog.sql — cached per-provider model catalog.
--
-- Models advertised by each provider's /models endpoint are cached here so a
-- model picker does not hit the upstream provider on every request. The catalog
-- is refreshed lazily with a TTL; rows cascade away with their provider.

create table if not exists models (
    provider_id uuid not null references providers(id) on delete cascade,
    id          text not null,
    owned_by    text,
    fetched_at  timestamptz not null default now(),
    primary key (provider_id, id)
);