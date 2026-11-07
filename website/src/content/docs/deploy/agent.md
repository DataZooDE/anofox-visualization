---
title: Agents and MCP
description: A dashboard's declared questions, answered over REST and MCP — deterministically, for the signed-in caller.
---

```sql
-- @vars supplier
SELECT name AS supplier ::DROPDOWN FROM suppliers ORDER BY 1;
-- @question How reliable is {supplier} on delivery?
SELECT name AS "Supplier" ::TABLE, on_time AS "On time %" FROM suppliers WHERE name = getvariable('supplier');
```

In **agent mode** a locked server answers only these declared questions: `GET /api/agent` lists them and their slots, `POST /api/agent/answer` answers one, and with `agent.mcp` an MCP server at `/mcp` offers one tool per question. No model is called: the question's panel runs through the panel path, under the caller's claims and the policy, and the answer is its rows as facts with a footer.

1. `dashboard --check --agent file.sql` — safe question texts and listed columns.
2. `dashboard --publish file.sql --out /srv/agent/<id>` — the bundle and its `sha256:` digest.
3. Start with `"agent": {"dashboard": "<id>", "bundle": "/srv/agent/<id>", "digest": "sha256:…"}`, a login, and an explicit policy.

The server refuses a bundle that isn't the served dashboard's, and with `agent.drift` it re-checks every question after each data refresh and switches off one whose data drifted. Customer mappings onto a canonical contract are checked by `dashboard --conform`.

Details: [Agent mode](https://github.com/DataZooDE/anofox-visualization/blob/master/docs/secure-serving.md#agent-mode-declared-questions-only-optionsagent-863).
