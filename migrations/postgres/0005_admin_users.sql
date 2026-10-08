-- 0005_admin_users.sql — account state for the admin console.
--
-- `disabled` lets an admin lock an account out (enforced on every authenticated
-- request by re-reading the row). `last_seen_at` is best-effort activity,
-- refreshed in the background so "active users" is meaningful without a write
-- on every request.

alter table users
    add column if not exists disabled boolean not null default false,
    add column if not exists last_seen_at timestamptz;

create index if not exists users_last_seen_idx on users (last_seen_at desc);