-- @title Shared ::ROWSELECT and input variables (#398)
-- @group Lint fixtures
--
-- `-- @shared <name>` documents a deliberately shared variable: a dropdown
-- and two ::ROWSELECT tables below all hand `material` to one detail panel
-- (fan-in + a dropdown/row-select hand-off) — the header silences
-- interaction/name-clash and interaction/rowselect-shared-name for just
-- that name. Two more tables share `name` by accident (no header): still
-- flagged. The last `-- @shared` names a variable its own ::ROWSELECT
-- doesn't set (a typo): flagged on its own (interaction/shared-unused).

CREATE OR REPLACE TABLE items AS
SELECT ['Widget', 'Gadget', 'Gizmo'][1 + i % 3] AS material,
       ['Alpha', 'Beta'][1 + i % 2] AS name,
       i AS id
FROM range(0, 9) t(i);

SELECT DISTINCT material ::DROPDOWN FROM items ORDER BY material;

-- @shared material
SELECT 'single'::ROWSELECT;
SELECT material ::TABLE, id AS "ID" FROM items;

-- @shared material
SELECT 'single'::ROWSELECT;
SELECT material ::TABLE, id * 2 AS "Doubled" FROM items;

SELECT * ::TABLE FROM items WHERE material = getvariable('material');

-- Accidental: no `-- @shared` here, so the clash is still reported.
SELECT 'single'::ROWSELECT;
SELECT name ::TABLE, id AS "ID" FROM items;

SELECT 'single'::ROWSELECT;
SELECT name ::TABLE, id * 3 AS "Tripled" FROM items;

-- @shared nmae
SELECT 'single'::ROWSELECT;
SELECT id ::TABLE, name FROM items;
