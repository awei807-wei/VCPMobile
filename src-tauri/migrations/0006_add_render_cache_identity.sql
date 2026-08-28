-- Migration 0006: 为渲染缓存增加内容身份与渲染器 schema 版本。
ALTER TABLE render_cache ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE render_cache ADD COLUMN renderer_schema_version INTEGER NOT NULL DEFAULT 0;
