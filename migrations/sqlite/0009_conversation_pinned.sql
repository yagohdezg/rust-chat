-- 0009_conversation_pinned.sql — let users pin conversations to the top.
--
-- SQLite has no `add column if not exists`; migrations run once so this is safe.

alter table conversations add column pinned integer not null default 0;
