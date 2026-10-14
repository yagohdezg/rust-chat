-- 0024_usage_records.sql — per-turn token usage.
--
-- One row per completed provider turn, attributed to the account that asked for
-- it and, when known, the conversation, provider and model it used. Reporting
-- rolls these up, so nothing is pre-aggregated here. Rows cascade away with
-- their user; a deleted conversation or provider only clears the reference.

create table if not exists usage_records (
    id                uuid primary key default gen_random_uuid(),
    user_id           uuid not null references users(id) on delete cascade,
    conversation_id   uuid references conversations(id) on delete set null,
    provider_id       uuid references providers(id) on delete set null,
    model             text not null,
    prompt_tokens     bigint not null default 0,
    completion_tokens bigint not null default 0,
    total_tokens      bigint not null default 0,
    created_at        timestamptz not null default now()
);
create index if not exists usage_records_user_idx on usage_records (user_id, created_at);
