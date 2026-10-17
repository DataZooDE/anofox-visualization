-- @title Embedded (serve --dashboards)
-- @param channel [web, app] = web
SELECT 'user ' || getvariable('__claim_user') || ' channel ' || getvariable('channel') ::LABEL;
