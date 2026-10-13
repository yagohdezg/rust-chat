-- 0017_mcp_servers.sql — per-user Model Context Protocol server configuration.
--
-- The Postgres schema created `mcp_servers` in 0001; SQLite never had it, so it
-- is created here with the `args`/`env` columns added to Postgres in 0016.
-- UUIDs are BLOBs, JSON (`args`/`env`) is TEXT, and `enabled` is an integer.

create table if not exists mcp_servers (
    id         blob primary key,
    user_id    blob references users(id) on delete cascade,
    name       text not null,
    transport  text not null,                 -- stdio | http
    command    text,                          -- for stdio
    url        text,                          -- for http
    args       text not null default '[]',
    env        text not null default '{}',
    enabled    integer not null default 1,
    created_at text not null
);

create index if not exists mcp_servers_user_idx on mcp_servers (user_id, created_at);
