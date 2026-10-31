# Plan: scheduled tasks, reports and alerts (#767, #768)

Status: **design note, waiting for the owner's decisions (§9)**. No code yet.
Builds on the locked server (`anofox_serve_dashboards`, docs/secure-serving.md),
the export API (#764: `GET /api/export/<id>.svg|png|pdf`, `…/<panel>.csv|json`)
and, for chat channels, the identity rules of
[plan-dashboard-to-agent.md](plan-dashboard-to-agent.md) §8.

**Goal.**
- *Tasks* (#767): SQL that runs on a schedule next to the dashboards, to load,
  transform or clean data and then refresh the snapshot.
- *Reports and alerts* (#768): a dashboard, a tab or a panel delivered by
  email, Slack, Teams or a webhook — on a schedule, or when an alert panel
  returns rows.

Reference: Shaper (`*.task.sql` with a `::SCHEDULE` first statement; scheduled
email; Slack/Teams notifications). We take the task shape and add what Shaper
leaves to the operator: reports with an explicit identity, alerts with state,
and the limits.

## 1. Tasks

```sql
-- tasks/load_downloads.task.sql
SELECT INTERVAL '1 hour' ::SCHEDULE;            -- or a TIMESTAMP (next run), or 'init'
CREATE OR REPLACE TABLE stage.downloads AS
  SELECT * FROM read_parquet('/data/inbox/*.parquet');
DELETE FROM stage.downloads WHERE week < current_date - INTERVAL 2 YEAR;
SELECT 1 ::REFRESH;                              -- rebuild the snapshot when done
```

- **Where:** `<dashboards dir>/tasks/*.task.sql`. Files on disk, written by
  the dashboard authors, reviewed like the dashboards (git). Never editable
  from a browser and never a viewer's input.
- **Schedule:** the first statement returns one `::SCHEDULE` value, as in
  Shaper: an `INTERVAL` (every …), a `TIMESTAMP` (the next run, computed
  again after each run, e.g. `date_trunc('day', now()) + INTERVAL 1 DAY + INTERVAL 2 HOUR`)
  or `'init'` (once at start). A cron string is not needed with these.
- **Where it runs (D1):** in its own connection to the *serving session's*
  database, not in the locked snapshot. The snapshot stays read-only and is
  rebuilt (`::REFRESH`, or `refresh_s`) to pick up what a task wrote.
- **What it may write (D2):** the attached databases named in
  `options.tasks.write` (e.g. `["stage"]`), all others read-only for tasks.
  `SET`, `PRAGMA`, `ATTACH`/`DETACH`, `INSTALL`/`LOAD` and `COPY … TO` outside
  `options.data` are refused (the same statement checks the locked server
  already makes for the data folder).
- **One at a time:** tasks run one after another on one worker thread, each
  with a timeout (`options.tasks.timeout_s`, default 15 min) and its own
  memory limit, never more than one run of the same task at once.
- **Failure:** logged, retried at the next tick; `anofox_task_*` metrics
  (#765: last success, consecutive failures, duration) and an optional alert
  route (§3).
- **History:** one JSON line per run (`options.tasks.log`): task, start, end,
  rows changed where DuckDB reports them, the error (redacted like a panel's).
- **Replicas (D3):** tasks run only where `options.tasks.run: true`: one
  replica is the task runner, the others only serve and refresh. No
  election — explicit is simpler and can't run a task twice.
- **Lint:** `task/*` — no `::SCHEDULE` first, an unparseable schedule, a
  write to a database not in `write`, a refused statement.

## 2. Reports

A report is operator configuration, not dashboard content: who receives what
is decided by the person who runs the server.

```json
{"reports": [
  {"name": "weekly-sales", "dashboard": "sales", "tab": "Overview",
   "vars": {"region": "EU"}, "formats": ["png", "pdf"],
   "schedule": "every monday 07:00 Europe/Berlin",
   "as": {"user": "report:sales-eu", "groups": ["sales"], "claims": {"region": "EU"}},
   "to": [{"email": "sales-eu@example.com"}, {"slack": "C0123456"}]}
]}
```

- **Content:** rendered with the export path (#764), so every panel goes
  through the policy, the claims and the limits as a viewer's page would.
  The PNG goes inline and the PDF, CSV or JSON as attachments. The text has
  the title, the as-of time (`::ASOF`) and, if `options.public_url` is set, a
  link to the dashboard (the link needs a login like any other).
- **Identity (D4):** the `as` principal: fixed, least-privilege claims for
  everyone on `to`, like the chat channel principal in
  plan-dashboard-to-agent §8. The operator attests that every recipient may
  see what the principal can see; the start message prints each report's
  principal. `report:*` users are reserved (no login method can claim them)
  and never submit or save.
- **Schedule:** the same three forms as tasks, plus a short readable form
  for reports (`every monday 07:00 Europe/Berlin`, `every day 06:30`,
  `every hour`), turned into the next timestamp in the server.
- **No viewer can trigger, change or redirect a report.** The admin endpoint
  `POST /api/reports/<name>/run` (an API key with a `reports` scope) is the
  only manual trigger, for testing.

## 3. Alerts

```json
{"alerts": [
  {"name": "stock-below-safety", "dashboard": "stock", "panel": "below_safety",
   "check": "every 15 minutes", "as": {"user": "report:ops"},
   "attach": ["csv"], "to": [{"slack": "C0OPS"}],
   "repeat_after": "24 hours"}
]}
```

- The alert **fires when the named panel returns rows**, and resolves when it
  returns none. The panel is ordinary dashboard SQL (`WHERE stock < safety`),
  so the condition is reviewed like any panel.
- **State (D5):** an alert is sent on a change, from none to some rows, or
  when the set of rows changes (by a hash of the rows). If it stays firing,
  it is sent again after `repeat_after`, and a "resolved" message is sent once
  when it clears. State is kept in a small JSON file, so a restart doesn't
  send the alert again.
- A failed task can route to an alert channel too
  (`options.tasks.on_failure: {"to": […]}`).

## 4. Channels

| Channel | How | Notes |
|---|---|---|
| Email | SMTP with STARTTLS required (or implicit TLS); `smtp.host`, `port`, `user_env`, `password_env`, `from` | inline PNG (`cid:`), attachments; one message per report, recipients in `Bcc` |
| Slack | bot token (`token_env`), `chat.postMessage` + `files.uploadV2` | files uploaded as bytes, never a URL Slack fetches; channel ids only; refused for channels with external members (Slack Connect) unless `allow_external: true` |
| Teams | Workflows (Power Automate) webhook URL (`url_env`) | an Adaptive Card with the text and the PNG (≤ 1 MB, else a link only); attachments only through email |
| Webhook | `POST` JSON to `url_env`, `HMAC-SHA256` signature header with `secret_env` | body: report name, time, text, files base64; for anything else (Teams via a relay, n8n, …) |

- All secrets come from the environment, never from a dashboard or the
  options JSON itself.
- Outbound HTTPS only, with no redirects to another host, a 30 s timeout,
  and the system CA store (or `ca_file`).
- **Limits:** at most 50 recipients per report, 10 MB of attachments, a
  global rate (`options.reports.max_per_hour`, default 60). Over a limit the
  report is not sent, it is logged and the metric is raised; it never sends
  a truncated report.

## 5. Security (to review before code)

- **New capabilities:** tasks are the first writes on a locked server, and
  reports are the first outbound connections. Both are off unless
  configured, and both are operator configuration plus author SQL, never
  viewer input.
- **Data leaving the server:** every report and alert uses an explicit
  principal, through the same policy path as a page. Rendering is
  deterministic and no model is involved.
- **SSRF / exfiltration:** channel endpoints are fixed in the options
  (env-resolved), never built from data. Task SQL may read files and URLs as
  DuckDB allows; the `httpfs` and `http_client` loads are the operator's
  choice (`options.load`).
- **Injection into messages:** report text is built from fixed templates and
  escaped per channel (HTML for email, mrkdwn escaping for Slack,
  Adaptive Card text for Teams). Cell values never become markup or links.
- **Spoofing and replay:** webhook bodies are HMAC-signed with a timestamp.
  Email `From` is the configured sender, with SPF/DKIM the operator's.
- A threat-model table goes into docs/secure-serving.md with the first
  slice.

## 6. Slices

1. **T1 tasks:** the scheduler, `::SCHEDULE`, the write allowlist, the
   history, `::REFRESH`, metrics and lint. No outbound traffic.
2. **R1 reports to email and webhook:** the `reports` options, the principal,
   rendering through #764, the SMTP and webhook senders, and the manual run
   endpoint.
3. **R2 alerts:** state, repeat and resolve, and task-failure routing.
4. **R3 Slack and Teams.**

Each slice ships with its docs, lint, metrics and an end-to-end test against
local fakes (an SMTP sink, a webhook receiver, a fake Slack API). No test
sends real mail or posts to real Slack.

## 7. Not in scope

- A browser UI for tasks or reports (they're files and options, edited in git).
- Per-recipient identities for reports (one principal per report; send two
  reports for two audiences).
- Distributed scheduling or leader election (D3).

## 8. Open points for later

- Whether `serve --dashboards` (the static server) gets tasks too. For now,
  locked server only.
- A "send me this" button in the page for a signed-in viewer (their own
  identity, to their own verified e-mail) — a separate issue if wanted.

## 9. Decisions for the owner (recommendation after the arrow)

- **D1** Where tasks run → their own connection to the serving session's
  database, never in the snapshot; `::REFRESH` rebuilds the snapshot after.
- **D2** What tasks may write → only the attached databases named in
  `options.tasks.write`; no `SET`/`PRAGMA`/`ATTACH`/`INSTALL`/`LOAD`.
- **D3** Replicas → one explicit task runner (`options.tasks.run: true`), no
  election.
- **D4** Report identity → one explicit principal per report (`as`), with the
  operator attesting it fits every recipient; reserved `report:*` users.
- **D5** Alert state → fire on a change, `repeat_after`, a "resolved"
  message, kept in a local state file.
- **D6** Channels and order → email and webhook first (R1), Slack and Teams
  after (R3).
- **D7** Configuration → reports and alerts in the server options (operator
  config), tasks as `*.task.sql` files (author code).
- **D8** Schedules → `INTERVAL` / `TIMESTAMP` / `'init'` for tasks, plus the
  short readable form (`every monday 07:00 Europe/Berlin`) for reports and
  alerts.
