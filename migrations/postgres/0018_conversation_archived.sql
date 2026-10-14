-- 0018_conversation_archived.sql — let users archive conversations.
--
-- Archived chats are hidden from the default listing (see `list_conversations`)
-- but retained; the flag is per-conversation and defaults off.

alter table conversations add column if not exists archived boolean not null default false;
