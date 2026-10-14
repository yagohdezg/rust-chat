-- 0023_provider_vision.sql — providers that can serve vision models.
--
-- SQLite has no `add column if not exists`; migrations run once so this is safe.
-- `vision` is an integer flag (0/1) set from the provider form; it decides
-- whether attached images are forwarded to the model or read as text.

alter table providers add column vision integer not null default 0;
