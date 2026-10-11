-- 0014_computer_pool.sql — durable ownership for the warm box pool.
--
-- A warm pool pre-provisions blank boxes to hide BoxLite cold start. The boxes
-- themselves live on `sandboxd` nodes, but ownership is recorded here so a
-- chat-server restart or crash can reconcile: every tracked handle is a box
-- this deployment owns, and a pool box found on a node that is not tracked is
-- a leak the reconciler reclaims. Without this table the pool lived only in
-- process memory and every crash orphaned its boxes.

create table if not exists computer_pool (
    handle     text primary key,      -- backend box id on the node
    node       text not null,
    created_at timestamptz not null default now()
);

create index if not exists computer_pool_node_idx on computer_pool (node);
