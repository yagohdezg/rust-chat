alter table providers add column if not exists allowed_models jsonb not null default '[]'::jsonb;
alter table users add column if not exists allowed_models jsonb not null default '[]'::jsonb;
