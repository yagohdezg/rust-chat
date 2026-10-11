-- 0013_computer_workspace.sql — durable per-computer workspace volume.
--
-- Each computer mounts a managed BoxLite volume at its workspace, so files
-- survive the box being destroyed and recreated (idle reaping, node changes,
-- process restarts). The name is recorded so a re-provisioned box can remount
-- the same volume and an explicit destroy can reclaim it.

alter table computers add column if not exists volume text;
