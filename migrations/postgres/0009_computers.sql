-- 0009_computers.sql — per-user persistent "computer" placement registry.
--
-- A computer is a long-lived, per-user workspace (a BoxLite microVM today)
-- that survives chat sessions. This table is the placement registry the
-- control plane routes through: it records which node hosts a user's
-- computer and its lifecycle state, so routing survives process restarts and
-- does not depend on a sticky load balancer.
--
-- States: provisioning | running | paused | destroyed. At most one live
-- (non-destroyed) computer exists per user, enforced by the partial unique
-- index below.

create table if not exists computers (
    id             uuid primary key default gen_random_uuid(),
    user_id        uuid not null references users(id) on delete cascade,
    node           text not null,
    handle         text,                             -- backend box id on the node
    state          text not null default 'provisioning',
    created_at     timestamptz not null default now(),
    updated_at     timestamptz not null default now(),
    last_active_at timestamptz not null default now()
);

create unique index if not exists computers_user_live_idx
    on computers (user_id) where state <> 'destroyed';

create index if not exists computers_node_idx
    on computers (node) where state <> 'destroyed';

create index if not exists computers_idle_idx
    on computers (last_active_at) where state = 'running';
