-- @title Other dashboard
SELECT 'other dashboard for ' || coalesce(getvariable('__claim_user'), '(nobody)') ::LABEL;
