-- 0019_conversation_archived.sql — let users archive conversations.
--
-- SQLite has no `add column if not exists`; migrations run once so this is safe.

alter table conversations add column archived integer not null default 0;
