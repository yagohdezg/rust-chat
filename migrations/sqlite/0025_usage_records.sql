-- 0025_usage_records.sql — per-turn token usage.
--
-- Mirrors the Postgres `0024_usage_records.sql` columns. One row per completed
-- provider turn, attributed to the account that asked for it and, when known,
-- the conversation, provider and model it used. Reporting rolls these up, so
-- nothing is pre-aggregated here.

create table if not exists usage_records (
    id                blob primary key,
    user_id           blob not null references users(id) on delete cascade,
    conversation_id   blob references conversations(id) on delete set null,
    provider_id       blob references providers(id) on delete set null,
    model             text not null,
    prompt_tokens     integer not null default 0,
    completion_tokens integer not null default 0,
    total_tokens      integer not null default 0,
    created_at        text not null
);
create index if not exists usage_records_user_idx on usage_records (user_id, created_at);
