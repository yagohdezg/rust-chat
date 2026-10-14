-- 0021_auth_tokens.sql — email verification and password reset tokens.
--
-- Mirrors the Postgres `0020_auth_tokens.sql` columns. SQLite has no
-- `add column if not exists`; migrations run once so separate statements are
-- safe. `email_verified` is an integer flag (0/1).

alter table users add column email_verified integer not null default 0;

-- Accounts that predate the feature keep working when an operator turns on
-- REQUIRE_EMAIL_VERIFICATION; only new sign-ups start out unconfirmed.
update users set email_verified = 1 where email_verified = 0;

create table if not exists auth_tokens (
    id         blob primary key,
    user_id    blob not null references users(id) on delete cascade,
    kind       text not null,
    token_hash text not null unique,
    expires_at text not null,
    used_at    text,
    created_at text not null
);
create index if not exists auth_tokens_user_idx on auth_tokens (user_id, kind);
