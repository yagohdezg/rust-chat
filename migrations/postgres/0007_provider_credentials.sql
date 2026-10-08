-- 0007_provider_credentials.sql — per-user API keys for shared providers.
--
-- An admin can provision an instance-wide provider (providers.user_id IS NULL)
-- without an API key; each user then stores their own key here. Key resolution
-- prefers the user's credential over the provider's own key. Cascades with both
-- the user and the provider.

create table if not exists provider_credentials (
    user_id     uuid not null references users(id) on delete cascade,
    provider_id uuid not null references providers(id) on delete cascade,
    api_key     text not null,
    created_at  timestamptz not null default now(),
    primary key (user_id, provider_id)
);
