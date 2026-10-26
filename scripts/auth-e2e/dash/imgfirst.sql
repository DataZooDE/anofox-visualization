-- @title Image first (#554 S1 regression)
-- A relative assets/ ::IMAGE as the dashboard's *first* statement, with a
-- panel after it that needs its own /api/panel round trip. p3.mjs's own
-- embed case (dash/rls.sql) put its ::IMAGE last, which is why that case
-- alone didn't catch S1: the image panel is built into the still-detached
-- newGrid while later panels' awaits are still in flight, so its blob:
-- fetch has every chance to resolve *before* the swap into `.dash` — the
-- common case the fix handles, not a rare race (see web/app.js's own
-- comment on mkImg and the swap-time sweep).
SELECT 'assets/rls-logo.svg' ::IMAGE, 'Company mark' ::ALT;
SELECT 1 ::METRIC, 'Later panel' ::LABEL;
