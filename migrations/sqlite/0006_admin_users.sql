-- 0006_admin_users.sql — account state for the admin console.
--
-- Mirrors the Postgres `0005_admin_users.sql` columns. SQLite has no
-- `add column if not exists`; migrations run once so separate statements are
-- safe. `disabled` is an integer flag (0/1).

alter table users add column disabled integer not null default 0;
alter table users add column last_seen_at text;

create index if not exists users_last_seen_idx on users (last_seen_at desc);