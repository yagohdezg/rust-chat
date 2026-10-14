# Target EKS topology

This is a **target design**, not a description of running infrastructure. The
EKS manifests for milestone M5 do not exist yet; when they are written they must
match this document. It fixes where each workload runs, what it scales on, and
how traffic and state are wired, using only the settings the code already reads
(see [`.env.example`](../.env.example) and
[`crates/chat-core/src/config.rs`](../crates/chat-core/src/config.rs)).

## Scope

The workloads to place:

| Workload | What it is | Process |
| --- | --- | --- |
| `web` | SvelteKit UI, built with `VITE_API_BASE` | Node |
| `chat-server` | axum HTTP API, SSE chat, agent runtime | Rust |
| `sandboxd` | Sandbox exec service (`POST /v1/exec`, `/v1/computers*`) | Rust |
| `boxlite` | `boxlite serve`, one microVM per box | Rust (privileged) |
| `migrate` | One-shot schema migration | Rust (`chat-server migrate`) |

`chat-server` holds no in-process sandbox backend: it always calls `sandboxd`
over HTTP (`SANDBOXD_URL`, `SANDBOXD_TOKEN`) and fails closed if `sandboxd` is
unreachable.

## Cluster shape

- One EKS cluster per environment, VPC with public and private subnets across at
  least three AZs. Worker nodes run in private subnets with no public IPs.
- Managed node groups, one per workload class below, so that a sandbox node
  churning or rebooting never evicts API pods, and vice versa.
- An AWS Load Balancer Controller owns a single internet-facing ALB for ingress;
  `web` and `chat-server` are the only services behind it.
- IRSA (IAM roles for service accounts) supplies S3 credentials to
  `chat-server`; the S3 client also reads the standard `AWS_ACCESS_KEY_ID` /
  `AWS_SECRET_ACCESS_KEY` variables or an instance profile.

## Node groups

| Node group | Hosts | Instance class | Why it is separate |
| --- | --- | --- | --- |
| `system` | CoreDNS, ALB controller, cluster add-ons, the `migrate` Job | small general-purpose, on-demand | Control-plane-adjacent pods must not be starved by chat load or sandbox churn |
| `web` | `web`, `chat-server` | general-purpose, on-demand or spot | Stateless; scales fast and often |
| `sandbox-kvm` | `boxlite`, `sandboxd` | KVM-capable (metal or `*.metal`), on-demand | BoxLite boots a hardware-isolated microVM per box and needs `/dev/kvm`; these nodes are expensive and churn slowly |
| `sandbox-gvisor` | `sandboxd` | general-purpose, no KVM | Middle tier for environments without KVM-capable nodes |

The `boxlite` container runs privileged with `/dev/kvm` and `/dev/net/tun`
passed through (as in [`deploy/compose.yaml`](../deploy/compose.yaml)). The
`sandbox-kvm` group is the only group that needs those devices.

`gVisor` is the **no-KVM middle tier**: a user-space kernel intercepts syscalls
with less isolation than a microVM but without the KVM host requirement. The
crate already models this as `IsolationLevel::UserSpaceKernel`
([`crates/chat-sandbox/src/lib.rs`](../crates/chat-sandbox/src/lib.rs)), but no
gVisor backend exists yet — it is a target tier, not a shipped one. Today
`sandbox-kvm` (BoxLite) is the only working sandbox tier.

## Autoscaling

The two tiers scale on **different signals** and must be scaled independently.
`web`/`chat-server` react to request load; the sandbox tier reacts to how much
sandbox work is queued, and its nodes are far slower and more expensive to add.

### Web / API tier

- `chat-server` is stateless across replicas. Two pieces of state must be
  external for more than one replica:
  - **Files**: `FILE_STORAGE_KIND=s3`. A pod's local disk is invisible to its
    siblings, so `local` is correct only for a single replica.
  - **Relational data**: an external Postgres (`DATABASE_URL`), not a
    per-pod SQLite file.
- Scale with a HorizontalPodAutoscaler on **CPU plus request rate** (RPS from
  the ALB target group or ingress request count), with a floor of two replicas
  for availability and a rolling update strategy.
- No sticky sessions are needed. SSE rejoin works within a single replica;
  reconnecting to a different replica replays the persisted message content and
  reports an interruption if generation was still in flight, because replies are
  checkpointed to the database and generation runs in a detached task.
- Note: the per-IP rate limiter (`RATE_LIMIT_ENABLED`) is in-memory and
  per-replica, so its effective limit scales with the replica count. Keep the
  per-replica limits in mind when sizing, or move limiting to the edge if a hard
  global limit is needed.

### Sandbox tier

- Scale on **pending sandbox work**, not CPU: the number of pending
  executions / pending computer provisions (queue depth), plus the count of live
  computers. CPU on a KVM node is a poor signal because a mostly idle node can
  still be saturated by one booting microVM.
- Scale the `sandbox-kvm` group separately from `web`, with a longer
  scale-down stabilization window and scale-up that tolerates slow node joins.
  KVM nodes are expensive and churn slowly, so prefer over-provisioning a spare
  node or a warm pool over aggressive scale-to-zero.
- `COMPUTER_WARM_POOL` pre-provisions blank boxes per node to hide BoxLite cold
  start. `COMPUTER_IDLE_TTL_SECONDS` and `COMPUTER_REAP_INTERVAL_SECONDS`
  bound how long idle computers hold a node's memory.
- Placement is by **node name**, not a sticky load balancer: the registry stores
  the node a computer lives on, and `SANDBOX_NODES` maps
  `name=url[,name=url...]` to callable `sandboxd` endpoints. New computers go to
  the least-loaded node. Because a computer is pinned to a node name, each
  `sandboxd` needs a **stable identity** (for example one StatefulSet pod per
  sandbox node, selected by node affinity) — ephemeral Deployment pods with
  rotating names would leave placements dangling.

## Networking

- **Ingress**: one internet-facing ALB in the public subnets routes to `web`
  and `chat-server` target groups. Nothing else is publicly reachable.
- **Private subnets**: worker nodes and all backing services (Postgres, the
  sandbox nodes) live in private subnets.
- **`sandboxd` is internal only**: no ingress, no public endpoint. It is reached
  by `chat-server` at `SANDBOXD_URL` over the cluster network and authenticates
  every call with `Authorization: Bearer <SANDBOXD_TOKEN>` (`/health` is open).
  `SANDBOXD_TOKEN` is mandatory — `sandboxd` refuses to start without it.
- **Sandbox egress is denied by default.** A BoxLite box has no outbound network
  unless `SANDBOX_NETWORK=true`; when enabled, `SANDBOX_EGRESS_ALLOW`
  (comma-separated IPv4, CIDR, hostname or `*.wildcard`, at most 10) restricts it
  to an allow-list and sinkholes everything else. The policy applies to every box
  a `sandboxd` creates, including per-user computers, so it is a per-node /
  per-deployment control. `SANDBOX_EGRESS_ALLOW` requires `SANDBOX_NETWORK=true`.
- **S3 access** goes through a gateway VPC endpoint so file traffic does not
  traverse NAT.
- **Postgres** sits behind a security group that admits only `chat-server` and
  the `migrate` Job.

## State

| State | Backing | Settings |
| --- | --- | --- |
| Relational data | Managed PostgreSQL 16 with the `pgvector` extension (RDS/Aurora) | `DB_BACKEND=postgres`, `DATABASE_URL` |
| Uploaded files | S3 (or an S3-compatible store) | `FILE_STORAGE_KIND=s3`, `S3_BUCKET`, `S3_REGION`, optional `S3_ENDPOINT`/`S3_PREFIX` |
| Sandbox workspaces | Durable per-user volumes + BoxLite image/kernel cache on the sandbox nodes | `COMPUTER_WORKSPACE=true`; persistent EBS attached to `sandbox-kvm` |
| Secrets | Secrets Manager / External Secrets | `JWT_SECRET`, `SECRET_ENCRYPTION_KEY`, `SANDBOXD_TOKEN` |

- The Postgres image in the compose file is `pgvector/pgvector:pg16`; the managed
  instance must likewise have `pgvector` available.
- `SECRET_ENCRYPTION_KEY` encrypts provider API keys at rest. Set it explicitly
  in production so rotating `JWT_SECRET` does not strand stored keys.
- With `COMPUTER_WORKSPACE` (default on), each computer mounts a durable volume
  named from the user, so a workspace survives the box being stopped, reaped or
  re-provisioned on another node. The sandbox nodes therefore need persistent
  block storage, not just ephemeral instance storage.

## Migrations

- The `chat-server` Deployment runs with `MIGRATE_ON_BOOT=false`, so booting
  replicas never race each other applying schema changes.
- Migrations run once, out of band, as a one-shot Job: `chat-server migrate`
  (the same image, the `migrate` argument) applies migrations and exits. Wire it
  as a pre-sync / pre-upgrade hook (Argo CD or Helm) that must complete before
  the Deployment rolls.

## Observability (M5)

Milestone M5 owns the concrete metrics, logs and traces stack. This document
only fixes the signals each tier must expose so the autoscalers have inputs:

- **Health**: `chat-server` exposes `/health` for liveness and readiness;
  `sandboxd` exposes `/health` (unauthenticated).
- **Web/API scaling signal**: CPU and request rate per `chat-server` replica.
- **Sandbox scaling signal**: pending executions / queue depth and live computer
  count. `GET /api/admin/computers` already reports placements and warm-pool size
  per node.
- **Logs**: `tracing`, filtered by `LOG_LEVEL` (`chat-server`) and `RUST_LOG`
  (`sandboxd`).

## Configuration map

| Concern | Variables |
| --- | --- |
| Web/API replicas | `BIND_ADDR`, `DATABASE_URL`, `FILE_STORAGE_KIND=s3`, `S3_*`, `JWT_SECRET`, `SECRET_ENCRYPTION_KEY`, `CORS_ALLOWED_ORIGINS` |
| Sandbox nodes | `SANDBOX_NODES`, `SANDBOXD_URL`, `SANDBOXD_TOKEN`, `SANDBOX_IMAGE`, `SANDBOX_TIMEOUT_SECONDS`, `SANDBOX_MEMORY_MB`, `SANDBOX_CPUS`, `SANDBOX_DISK_GB` |
| Sandbox egress | `SANDBOX_NETWORK`, `SANDBOX_EGRESS_ALLOW` |
| Computers | `COMPUTERS_ENABLED`, `COMPUTER_WORKSPACE`, `COMPUTER_WARM_POOL`, `COMPUTER_IDLE_TTL_SECONDS`, `COMPUTER_REAP_INTERVAL_SECONDS` |
| Migrations | `MIGRATE_ON_BOOT=false`, one-shot `chat-server migrate` |
