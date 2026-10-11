-- 0014_computer_workspace.sql — durable per-computer workspace volume.
--
-- Mirrors the Postgres `0013_computer_workspace.sql` column. See that file
-- for the design notes.

alter table computers add column volume text;
