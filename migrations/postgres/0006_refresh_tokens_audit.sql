-- 0006_refresh_tokens_audit.sql — rotating refresh tokens and the audit trail.
--
-- `refresh_tokens` stores only a SHA-256 hash of each opaque token. Access
-- tokens stay short-lived JWTs; a refresh rotates the row (the old hash is
-- revoked) so a stolen token is single-use. Rows cascade away with their user.
--
-- `audit_logs` is an append-only record of security-relevant events (auth and
-- sandbox exec). `actor_id` is nulled rather than cascaded so history survives
-- account deletion; `metadata` carries action-specific JSON.

create table if not exists refresh_tokens (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid not null references users(id) on delete cascade,
    token_hash text not null unique,
    expires_at timestamptz not null,
    created_at timestamptz not null default now(),
    revoked_at timestamptz
);
create index if not exists refresh_tokens_user_idx on refresh_tokens (user_id);

create table if not exists audit_logs (
    id          uuid primary key default gen_random_uuid(),
    actor_id    uuid references users(id) on delete set null,
    action      text not null,
    target_type text,
    target_id   text,
    metadata    jsonb,
    ip          text,
    created_at  timestamptz not null default now()
);
create index if not exists audit_logs_created_idx on audit_logs (created_at desc);
