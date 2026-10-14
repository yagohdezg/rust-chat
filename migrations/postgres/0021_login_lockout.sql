-- 0021_login_lockout.sql — brute-force lockout for local sign-ins.
--
-- `failed_login_count` accumulates consecutive failures and `locked_until`
-- holds the end of the lock-out window. A successful sign-in clears both, and
-- an admin can clear them early from the user console.

alter table users
    add column if not exists failed_login_count bigint not null default 0;

alter table users
    add column if not exists locked_until timestamptz;
