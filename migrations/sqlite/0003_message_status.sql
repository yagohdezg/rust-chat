-- 0003_message_status.sql — generation lifecycle for streamed assistant replies.
--
-- SQLite has no `add column if not exists`; migrations run once so this is safe.

alter table messages add column status text not null default 'complete';
