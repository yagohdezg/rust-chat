-- 0008_conversation_pinned.sql — let users pin conversations to the top.
--
-- Pinned chats sort before the rest (see `list_conversations`); the flag is
-- per-conversation and defaults off.

alter table conversations add column if not exists pinned boolean not null default false;
