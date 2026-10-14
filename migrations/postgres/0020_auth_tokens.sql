-- 0020_auth_tokens.sql — email verification and password reset tokens.
--
-- Single-use, time-limited tokens for the email flows. Only the SHA-256 hash is
-- stored, so a database leak does not yield usable links; `kind` separates
-- verification from reset and `used_at` records consumption. `email_verified`
-- tracks whether the account's address has been confirmed.

alter table users
    add column if not exists email_verified boolean not null default false;

-- Accounts that predate the feature keep working when an operator turns on
-- REQUIRE_EMAIL_VERIFICATION; only new sign-ups start out unconfirmed.
update users set email_verified = true where email_verified = false;

create table if not exists auth_tokens (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid not null references users(id) on delete cascade,
    kind       text not null,
    token_hash text not null unique,
    expires_at timestamptz not null,
    used_at    timestamptz,
    created_at timestamptz not null default now()
);
create index if not exists auth_tokens_user_idx on auth_tokens (user_id, kind);
