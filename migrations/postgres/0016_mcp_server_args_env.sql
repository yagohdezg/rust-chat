-- 0016_mcp_server_args_env.sql — launch arguments and environment for MCP servers.
--
-- A stdio MCP server is spawned from a command plus its arguments and
-- environment, so both are persisted alongside it. `args` is an ordered JSON
-- array of strings; `env` is a JSON object of string values.

alter table mcp_servers
    add column if not exists args jsonb not null default '[]'::jsonb,
    add column if not exists env jsonb not null default '{}'::jsonb;
