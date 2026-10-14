-- 0022_login_lockout.sql — brute-force lockout for local sign-ins.
--
-- Mirrors the Postgres `0021_login_lockout.sql` columns. SQLite has no
-- `add column if not exists`; migrations run once so separate statements are
-- safe.

alter table users add column failed_login_count integer not null default 0;
alter table users add column locked_until text;
