-- 0016_message_reasoning.sql — reasoning/thinking text for streamed assistant replies.
--
-- SQLite has no `add column if not exists`; migrations run once so this is safe.

alter table messages add column reasoning text;
