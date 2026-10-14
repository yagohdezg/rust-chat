alter table providers add column allowed_models text not null default '[]';
alter table users add column allowed_models text not null default '[]';
