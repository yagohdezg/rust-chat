-- 0022_provider_vision.sql — providers that can serve vision models.
--
-- `vision` is set from the provider form; it decides whether attached images
-- are forwarded to the model as image parts or read as text.

alter table providers
    add column if not exists vision boolean not null default false;
