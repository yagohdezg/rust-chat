-- 0010_computers.sql — per-user persistent "computer" placement registry.
--
-- Mirrors the Postgres `0009_computers.sql` table. UUIDs are BLOBs and
-- timestamps are RFC3339 TEXT. See that file for the design notes.

create table if not exists computers (
    id             blob primary key,
    user_id        blob not null references users(id) on delete cascade,
    node           text not null,
    handle         text,
    state          text not null default 'provisioning',
    created_at     text not null,
    updated_at     text not null,
    last_active_at text not null
);

create unique index if not exists computers_user_live_idx
    on computers (user_id) where state != 'destroyed';

create index if not exists computers_node_idx
    on computers (node) where state != 'destroyed';

create index if not exists computers_idle_idx
    on computers (last_active_at) where state = 'running';
