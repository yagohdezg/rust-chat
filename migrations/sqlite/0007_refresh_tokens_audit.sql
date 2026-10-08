-- 0007_refresh_tokens_audit.sql — rotating refresh tokens and the audit trail.
--
-- Mirrors the Postgres `0006_refresh_tokens_audit.sql` columns. UUIDs are BLOBs,
-- timestamps are RFC3339 TEXT and JSON is TEXT, matching how `sqlx` encodes
-- `Uuid`, `DateTime<Utc>` and `Json<Value>` here.

create table if not exists refresh_tokens (
    id         blob primary key,
    user_id    blob not null references users(id) on delete cascade,
    token_hash text not null unique,
    expires_at text not null,
    created_at text not null,
    revoked_at text
);
create index if not exists refresh_tokens_user_idx on refresh_tokens (user_id);

create table if not exists audit_logs (
    id          blob primary key,
    actor_id    blob references users(id) on delete set null,
    action      text not null,
    target_type text,
    target_id   text,
    metadata    text,
    ip          text,
    created_at  text not null
);
create index if not exists audit_logs_created_idx on audit_logs (created_at desc);
