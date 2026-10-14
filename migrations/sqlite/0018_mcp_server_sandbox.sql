-- 0018_mcp_server_sandbox.sql — run an MCP server inside the sandbox.
--
-- `sandbox` selects whether a stdio server runs inside a BoxLite microVM
-- instead of on the host; `sandbox_image` optionally overrides the image it
-- runs in, defaulting to the deployment's SANDBOX_IMAGE.

alter table mcp_servers add column sandbox integer not null default 0;
alter table mcp_servers add column sandbox_image text;
