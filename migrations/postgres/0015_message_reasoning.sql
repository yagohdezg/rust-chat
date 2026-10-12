-- 0015_message_reasoning.sql — reasoning/thinking text for streamed assistant replies.
--
-- Reasoning models emit a separate reasoning stream before the answer. It is
-- persisted so the UI can render it after a reload or reconnect.

alter table messages
    add column if not exists reasoning text;
