-- 0011_conversation_files.sql — many-to-many attachments between a user's
-- personal file storage (`files`) and conversations.
--
-- `files` is the user's own library: a file persists independently of any
-- conversation and can be attached to any number of conversations (and thus
-- materialized into each one's sandbox). Uploading directly into a conversation
-- creates the library file and an attachment here.
--
-- The legacy `files.conversation_id` column is retained as the file's *origin*
-- for traceability but is no longer authoritative; this join table is.

create table if not exists conversation_files (
    conversation_id blob not null references conversations(id) on delete cascade,
    file_id         blob not null references files(id) on delete cascade,
    created_at      text not null,
    primary key (conversation_id, file_id)
);

create index if not exists conversation_files_file_idx
    on conversation_files (file_id);

-- Backfill attachments from the legacy origin column.
insert or ignore into conversation_files (conversation_id, file_id, created_at)
select conversation_id, id, created_at from files where conversation_id is not null;

-- Detach the legacy origin. `files.conversation_id` still carries an
-- `on delete cascade` FK, so leaving it populated would delete library files
-- when their *originating* conversation is removed. Attachments (the join
-- table) are authoritative now.
update files set conversation_id = null where conversation_id is not null;
