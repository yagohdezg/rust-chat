-- 0003_message_status.sql — generation lifecycle for streamed assistant replies.
--
-- `complete` covers all historical and non-assistant messages. Assistant rows
-- are created with `streaming` and updated as chunks arrive so a client can
-- resume after a disconnect.

alter table messages
    add column if not exists status text not null default 'complete';
