-- 0015_computer_pool.sql — durable ownership for the warm box pool.
--
-- Mirrors the Postgres `0014_computer_pool.sql` table. Timestamps are RFC3339
-- TEXT. See that file for the design notes.

create table if not exists computer_pool (
    handle     text primary key,
    node       text not null,
    created_at text not null
);

create index if not exists computer_pool_node_idx on computer_pool (node);
