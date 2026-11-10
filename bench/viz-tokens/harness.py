#!/usr/bin/env python3
"""#1044: talk to data and deliver a chart, three ways, one agent loop.

Every arm gets the same model and settings, the same data (one read-only
DuckDB file per dataset), the same question, the same exploration tools
(list_tables, describe, query) and the same ending (answer). Only the chart
tool and the reference in the system prompt differ:

  anofox    render(sql)          annotated SQL (::ROLE casts), drawn by `dashboard --svg`
  python    run_python(code)     pandas / matplotlib / duckdb, saves chart.png
  vegalite  render(sql, spec)    the query's rows go into the Vega-Lite spec, drawn by vl-convert

Every chart tool answers in text only (ok, or the error), so no arm sees its
picture. All model-written code runs in bubblewrap: read-only filesystem, a
private /tmp, no network, a time and memory limit.

  ANTHROPIC_API_KEY=… python harness.py --tasks pilot --reps 3 --out results/pilot
  python harness.py --dry --out /tmp/x      # plumbing check, no API (scripted turns)

See README.md.
"""
import argparse
import base64
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

import duckdb

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
API = "https://api.anthropic.com/v1/messages"
PRICE = {  # per MTok, as scripts/assist-eval.mjs
    "claude-sonnet-5": {"in": 2, "out": 10, "read": 0.2, "write": 2.5},
    "claude-opus-5-5": {"in": 4, "out": 20, "read": 0.2, "write": 5},
}
MAX_ROUNDS = 12  # model calls per phase
TOOL_TEXT_MAX = 4000
QUERY_ROWS = 50
EXEC_TIMEOUT = 60
EXEC_MEM = 4 << 30

INSTRUCTIONS = """You are a data analyst. The user asks a question about the data in a DuckDB database. Answer it with ONE chart and a short written answer.

How to work:
- Look at the data first if you need to (list_tables, describe, query). query returns at most 50 rows.
- Then make the chart with {chart_tool}. It answers "ok" or the error; you do not see the picture. Fix errors and call it again.
- Finish with answer(text): two to four sentences that answer the question with the key numbers. The last chart you made successfully is the one shown with it.
- The chart must answer the question on its own: a clear title, readable labels, the right chart type.
- Do not ask the user questions; make reasonable choices.

When the user follows up, change the chart as asked (call {chart_tool} again) and answer again.
"""

ARM_NOTES = {
    "anofox": """# Your chart tool: render(sql)

render takes anofox-visualization SQL: one or more DuckDB SELECT statements whose result columns carry ::ROLE casts (e.g. `region::XAXIS`, `sum(x) AS "Revenue"::BARCHART`, `'Revenue by region'::TITLE`). Each statement is one panel; for this task make one chart panel (a KPI or a short table beside it only if it really helps). The tables are in the current database. The reference below lists the roles and how to use them.
""",
    "python": """# Your chart tool: run_python(code)

run_python runs Python 3 with pandas, numpy, matplotlib and seaborn. A DuckDB connection to the data is open as `con` (read-only; e.g. `df = con.sql("SELECT …").df()`). Save the chart with `plt.savefig("chart.png", dpi=150, bbox_inches="tight")`; only chart.png is shown. It returns stdout, stderr and whether chart.png was written. No network, no other files. The reference below is the house style for matplotlib charts; its own file and pattern lookups are not available here, so apply what it says directly.
""",
    "vegalite": """# Your chart tool: render(sql, spec)

render runs `sql` (one DuckDB SELECT) and draws `spec`, a Vega-Lite v5 JSON object (as a JSON object, not a string), with the query's rows as its data (`data` is set for you; use the query's column names as fields). Layers, transforms and facets are fine. It answers "ok" or the error. The reference below is the house style, written for Altair (the Python API of Vega-Lite); apply the same conventions in the JSON spec. Its own file and pattern lookups are not available here.
""",
}

COMMON_TOOLS = [
    {"name": "list_tables", "description": "The tables in the database, with their row counts.",
     "input_schema": {"type": "object", "properties": {}}},
    {"name": "describe", "description": "A table's columns and types, and 5 sample rows.",
     "input_schema": {"type": "object", "properties": {"table": {"type": "string"}}, "required": ["table"]}},
    {"name": "query", "description": "Run one read-only DuckDB SELECT; at most 50 rows come back, as CSV.",
     "input_schema": {"type": "object", "properties": {"sql": {"type": "string"}}, "required": ["sql"]}},
    {"name": "answer", "description": "The final written answer (2-4 sentences, with the key numbers). Ends this turn.",
     "input_schema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
]
CHART_TOOLS = {
    "anofox": {"name": "render", "description": "Draw anofox-visualization SQL (SELECT statements with ::ROLE casts) as the chart. Returns ok or the errors.",
               "input_schema": {"type": "object", "properties": {"sql": {"type": "string"}}, "required": ["sql"]}},
    "python": {"name": "run_python", "description": "Run Python code that saves the chart to chart.png (`con` is an open read-only DuckDB connection). Returns stdout/stderr and whether chart.png exists.",
               "input_schema": {"type": "object", "properties": {"code": {"type": "string"}}, "required": ["code"]}},
    "vegalite": {"name": "render", "description": "Run `sql` and draw the Vega-Lite v5 `spec` with its rows as data. Returns ok or the error.",
                 "input_schema": {"type": "object", "properties": {"sql": {"type": "string"}, "spec": {"type": "object"}}, "required": ["sql", "spec"]}},
}
EXPLORE = {"list_tables", "describe", "query"}
# anofox-lean: the anofox arm with the --lean reference (anofox-reference.mjs)
BASE = {"anofox-lean": "anofox"}


def base(arm):
    return BASE.get(arm, arm)


def cost(model, u):
    p = PRICE.get(model, PRICE["claude-sonnet-5"])
    return (u.get("input_tokens", 0) * p["in"] + u.get("output_tokens", 0) * p["out"]
            + u.get("cache_read_input_tokens", 0) * p["read"] + u.get("cache_creation_input_tokens", 0) * p["write"]) / 1e6


def trim(png_bytes, pad=16):
    """The chart without its empty border (every arm alike): the judge and the
    report compare charts, not canvas sizes."""
    from io import BytesIO
    from PIL import Image, ImageChops
    im = Image.open(BytesIO(png_bytes)).convert("RGB")
    bg = Image.new("RGB", im.size, im.getpixel((0, 0)))
    box = ImageChops.difference(im, bg).convert("L").point(lambda v: 255 if v > 12 else 0).getbbox()
    if box:
        l, t, r, b = box
        im = im.crop((max(0, l - pad), max(0, t - pad), min(im.width, r + pad), min(im.height, b + pad)))
    out = BytesIO()
    im.save(out, "PNG")
    return out.getvalue()


def clip(s, n=TOOL_TEXT_MAX):
    return s if len(s) <= n else s[: n - 40] + f"\n… ({len(s) - n + 40} more characters cut)"


# ---------------------------------------------------------------- sandbox

def sandboxed(cmd, workdir, extra_ro=()):
    """Run cmd in bubblewrap (read-only /, private /tmp, writable workdir, no
    network) in a memory-capped systemd scope, with a time limit."""
    args = ["bwrap", "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/tmp",
            "--bind", str(workdir), str(workdir), "--chdir", str(workdir),
            "--unshare-net", "--unshare-pid", "--die-with-parent",
            "--setenv", "MPLCONFIGDIR", "/tmp", "--setenv", "HOME", "/tmp"]
    for p in extra_ro:
        args += ["--ro-bind", str(p), str(p)]
    # a cgroup cap, not RLIMIT_AS: V8 (vl-convert) reserves far more address
    # space than it uses and dies under an address-space limit
    full = ["systemd-run", "--user", "--scope", "-q", "-p", f"MemoryMax={EXEC_MEM}",
            "-p", "MemorySwapMax=0", "--"] + args + ["--"] + cmd
    try:
        r = subprocess.run(full, capture_output=True, text=True, timeout=EXEC_TIMEOUT)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return -1, "", f"timed out after {EXEC_TIMEOUT} s"


# ---------------------------------------------------------------- tools

class Tools:
    def __init__(self, arm, db, workdir, cfg):
        self.arm, self.db, self.work, self.cfg = base(arm), Path(db), Path(workdir), cfg
        self.con = duckdb.connect(str(self.db), read_only=True,
                                  config={"enable_external_access": False, "lock_configuration": True})
        self.chart = None  # (png bytes, artifact text) of the last successful chart
        self.n = 0

    def run(self, name, inp):
        try:
            if name == "list_tables":
                rows = self.con.execute("SELECT table_name, estimated_size FROM duckdb_tables() ORDER BY 1").fetchall()
                return True, "\n".join(f"{t} ({n} rows)" for t, n in rows)
            if name == "describe":
                t = inp["table"]
                cols = self.con.execute("SELECT column_name, data_type FROM information_schema.columns WHERE table_name = ? ORDER BY ordinal_position", [t]).fetchall()
                if not cols:
                    return False, f"no table {t!r}"
                sample = self.con.execute(f'SELECT * FROM "{t}" LIMIT 5').df().to_csv(index=False)
                return True, "\n".join(f"{c}: {d}" for c, d in cols) + "\n\nsample:\n" + sample
            if name == "query":
                df = self.con.execute(inp["sql"]).df()
                more = f"\n({len(df)} rows, first {QUERY_ROWS} shown)" if len(df) > QUERY_ROWS else ""
                return True, df.head(QUERY_ROWS).to_csv(index=False) + more
            if name == "render" and self.arm == "anofox":
                return self.render_anofox(inp["sql"])
            if name == "run_python" and self.arm == "python":
                return self.run_python(inp["code"])
            if name == "render" and self.arm == "vegalite":
                return self.render_vegalite(inp["sql"], inp.get("spec"))
            return False, f"unknown tool {name}"
        except (duckdb.Error, KeyError, TypeError, ValueError) as e:
            return False, f"{type(e).__name__}: {e}"

    def step_dir(self):
        self.n += 1
        d = self.work / f"step{self.n:02d}"
        d.mkdir()
        return d

    def render_anofox(self, sql):
        d = self.step_dir()
        # the data read-only, a writable in-memory database first in the path
        # for the model's own helper tables (#1044: `USE d` refused them)
        script = f"ATTACH '{self.db}' AS d (READ_ONLY);\nSET search_path = 'memory.main,d.main';\n{sql}\n"
        (d / "chart.sql").write_text(script)
        dash = self.cfg["dashboard"]
        extra = ["--boundaries", self.cfg["boundaries"]] if self.cfg.get("boundaries") else []
        rc, out, err = sandboxed([dash, "--check", "chart.sql", "--json", *extra], d)
        try:
            diags = json.loads(out).get("diagnostics", [])
        except ValueError:
            return False, clip(f"check failed: {err or out}")
        errors = [x for x in diags if x.get("severity") == "error"]
        if errors:
            return False, clip("\n".join(f"statement {x.get('stmt', 0) - 2}: {x.get('code')}: {x.get('message')}" for x in errors))
        rc, out, err = sandboxed([dash, "--svg", "chart.sql", "chart.svg", "--width", "900", *extra], d)
        if rc != 0 or not (d / "chart.svg").exists():
            return False, clip(f"render failed: {err or out}")
        # a script without a chart role draws an empty page: say so, as the
        # other arms fail without a chart.png / a mark (#1044 pilot)
        svg = (d / "chart.svg").read_text()
        # every panel is a nested <svg x=…>; none = no statement carried a
        # role (#1044: the first version only knew marks and KPI tiles and
        # refused a correct ::TABLE). A panel saying "No data" is what a
        # viewer would see, so the model hears it too.
        if '<svg x="' not in svg:
            return False, "nothing was drawn: no statement carries a chart role (e.g. `sum(x) AS \"Revenue\" ::BARCHART`)"
        if ">No data<" in svg:
            return False, "drawn, but a panel shows 'No data' (no rows, or values the chart could not place)"
        rc, _, err = sandboxed(["rsvg-convert", "chart.svg", "-o", "chart.png"], d)
        if rc != 0:
            return False, clip(f"render failed: {err}")
        self.chart = ((d / "chart.png").read_bytes(), sql)
        return True, "ok: drawn"

    def run_python(self, code):
        d = self.step_dir()
        pre = (f"import duckdb\ncon = duckdb.connect()\ncon.execute(\"ATTACH '{self.db}' AS d (READ_ONLY)\")\n"
               "con.execute(\"SET search_path = 'memory.main,d.main'\")\n"
               "import matplotlib\nmatplotlib.use('Agg')\n")
        (d / "main.py").write_text(pre + code + "\n")
        rc, out, err = sandboxed([self.cfg["python"], "main.py"], d)
        png = d / "chart.png"
        made = png.exists() and png.stat().st_size > 0
        msg = f"exit {rc}; chart.png {'written' if made else 'NOT written'}"
        if out.strip():
            msg += "\nstdout:\n" + out[-1500:]
        if err.strip():
            msg += "\nstderr:\n" + err[-2500:]
        if rc == 0 and made:
            self.chart = (png.read_bytes(), code)
            return True, clip(msg)
        return False, clip(msg)

    def render_vegalite(self, sql, spec):
        if not isinstance(spec, dict):
            return False, "spec must be a JSON object"
        d = self.step_dir()
        df = self.con.execute(sql).df()
        if len(df) > 5000:
            return False, f"the query returned {len(df)} rows; aggregate it (at most 5000)"
        for c in df.columns:  # dates and decimals as JSON
            if str(df[c].dtype).startswith("datetime"):
                df[c] = df[c].dt.strftime("%Y-%m-%dT%H:%M:%S")
        spec = dict(spec)
        spec["data"] = {"values": json.loads(df.to_json(orient="records", date_format="iso"))}
        spec.setdefault("$schema", "https://vega.github.io/schema/vega-lite/v5.json")
        (d / "spec.json").write_text(json.dumps(spec))
        drawer = "import json, vl_convert as vlc\nopen('chart.png','wb').write(vlc.vegalite_to_png(json.load(open('spec.json')), scale=1.5))\n"
        (d / "draw.py").write_text(drawer)
        rc, out, err = sandboxed([self.cfg["python"], "draw.py"], d)
        png = d / "chart.png"
        if rc != 0 or not png.exists():
            return False, clip(f"render failed: {(err or out)[-2500:]}")
        art = json.dumps({"sql": sql, "spec": {k: v for k, v in spec.items() if k != "data"}})
        self.chart = (png.read_bytes(), art)
        return True, "ok: drawn"


# ---------------------------------------------------------------- model

class Anthropic:
    def __init__(self, key, model, effort):
        self.key, self.model, self.effort = key, model, effort

    def __call__(self, system, tools, messages):
        body = {"model": self.model, "max_tokens": 16000, "system": system, "tools": tools,
                "messages": messages, "thinking": {"type": "adaptive"},
                "output_config": {"effort": self.effort}, "cache_control": {"type": "ephemeral"}}
        req = urllib.request.Request(API, data=json.dumps(body).encode(), method="POST", headers={
            "x-api-key": self.key, "anthropic-version": "2023-06-01", "content-type": "application/json"})
        for attempt in range(5):
            try:
                with urllib.request.urlopen(req, timeout=300) as r:
                    return json.loads(r.read())
            except urllib.error.HTTPError as e:
                if e.code in (429, 500, 502, 503, 529) and attempt < 4:
                    time.sleep(10 * (attempt + 1))
                    continue
                raise RuntimeError(f"API {e.code}: {e.read()[:500]!r}")

    def count(self, system, tools):
        body = {"model": self.model, "system": system, "tools": tools,
                "messages": [{"role": "user", "content": "x"}]}
        req = urllib.request.Request(API + "/count_tokens", data=json.dumps(body).encode(), method="POST", headers={
            "x-api-key": self.key, "anthropic-version": "2023-06-01", "content-type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as r:
            return json.loads(r.read())["input_tokens"]


class Scripted:
    """--dry: plays fixed turns per arm (no API) to check the plumbing."""
    model = "scripted"

    def __init__(self, arm):
        self.arm, self.i = arm, 0

    def __call__(self, system, tools, messages):
        chart = {
            "anofox": ("render", {"sql": "SELECT region::XAXIS, sum(units*price) AS \"Revenue\"::BARCHART, 'Revenue by region'::TITLE FROM sales GROUP BY 1 ORDER BY 2 DESC;"}),
            "python": ("run_python", {"code": "import matplotlib.pyplot as plt\ndf = con.sql('SELECT region, sum(units*price) AS revenue FROM sales GROUP BY 1 ORDER BY 2 DESC').df()\nplt.bar(df.region, df.revenue); plt.title('Revenue by region')\nplt.savefig('chart.png')"}),
            "vegalite": ("render", {"sql": "SELECT region, sum(units*price) AS revenue FROM sales GROUP BY 1", "spec": {"mark": "bar", "encoding": {"x": {"field": "region", "type": "nominal", "sort": "-y"}, "y": {"field": "revenue", "type": "quantitative"}}, "title": "Revenue by region"}}),
        }[self.arm]
        steps = [("list_tables", {}), ("query", {"sql": "SELECT region, sum(units*price) FROM sales GROUP BY 1"}),
                 ("render" if self.arm != "python" else "run_python", {"sql": "SELECT nope"} if self.arm != "python" else {"code": "raise SystemExit('boom')"}),
                 chart, ("answer", {"text": "EU leads."})]
        name, inp = steps[self.i % len(steps)]
        self.i += 1
        return {"content": [{"type": "tool_use", "id": f"t{self.i}", "name": name, "input": inp}], "stop_reason": "tool_use",
                "usage": {"input_tokens": 100, "output_tokens": 50, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}}


# ---------------------------------------------------------------- one run

def phase(model, system, tools, messages, kit, log):
    """Model calls until answer() or MAX_ROUNDS; returns the answer text (or None)."""
    failed_chart = False
    for _ in range(MAX_ROUNDS):
        t0 = time.time()
        msg = model(system, tools, messages)
        call = {"usage": msg.get("usage", {}), "stop_reason": msg.get("stop_reason"), "s": round(time.time() - t0, 2), "tools": []}
        log["calls"].append(call)
        messages.append({"role": "assistant", "content": msg["content"]})
        uses = [b for b in msg["content"] if b.get("type") == "tool_use"]
        if not uses:
            messages.append({"role": "user", "content": "Please finish with the answer tool."})
            continue
        results, answer = [], None
        for u in uses:
            if u["name"] == "answer":
                answer = u["input"].get("text", "")
                results.append({"type": "tool_result", "tool_use_id": u["id"], "content": "ok"})
                call["tools"].append({"name": "answer", "kind": "answer", "ok": True})
                continue
            ok, text = kit.run(u["name"], u["input"])
            kind = "explore" if u["name"] in EXPLORE else ("repair" if failed_chart else "chart")
            if u["name"] not in EXPLORE:
                failed_chart = not ok
            call["tools"].append({"name": u["name"], "kind": kind, "ok": ok,
                                  "in_chars": len(json.dumps(u["input"])), "out_chars": len(text)})
            results.append({"type": "tool_result", "tool_use_id": u["id"], "content": text, **({} if ok else {"is_error": True})})
        messages.append({"role": "user", "content": results})
        if answer is not None:
            return answer
    return None


def run_one(task, arm, rep, model, cfg, out_dir, refs):
    run_id = f"{task['id']}__{arm}__r{rep}"
    work = Path(tempfile.mkdtemp(prefix=run_id + "-", dir=cfg["workdir"]))
    db = Path(cfg["data"]) / f"{task['dataset']}.duckdb"
    kit = Tools(arm, db, work, cfg)
    chart_tool = CHART_TOOLS[base(arm)]["name"]
    system = [
        {"type": "text", "text": INSTRUCTIONS.format(chart_tool=chart_tool) + "\n" + ARM_NOTES[base(arm)]},
        {"type": "text", "text": refs[arm], "cache_control": {"type": "ephemeral"}},
    ]
    tools = COMMON_TOOLS + [CHART_TOOLS[base(arm)]]
    messages = [{"role": "user", "content": task["question"]}]
    rec = {"run": run_id, "task": task["id"], "dataset": task["dataset"], "arm": arm, "rep": rep,
           "model": getattr(model, "model", "?"), "phases": []}
    for ph, text in (("first", task["question"]), ("followup", task["followup"])):
        if ph == "followup":
            messages.append({"role": "user", "content": text})
            kit.chart = None  # the follow-up's chart is the one it draws
        log = {"phase": ph, "calls": []}
        t0 = time.time()
        try:
            log["answer"] = phase(model, system, tools, messages, kit, log)
        except RuntimeError as e:
            log["answer"], log["error"] = None, str(e)
        log["seconds"] = round(time.time() - t0, 1)
        if kit.chart:
            png = out_dir / "charts" / f"{run_id}__{ph}.png"
            png.write_bytes(trim(kit.chart[0]))
            log["chart"], log["artifact"] = str(png.relative_to(out_dir)), kit.chart[1]
        else:
            log["chart"], log["artifact"] = None, None
        u = [c["usage"] for c in log["calls"]]
        log["tokens"] = {k: sum(x.get(k, 0) for x in u) for k in ("input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens")}
        log["cost"] = round(sum(cost(rec["model"], x) for x in u), 5)
        rec["phases"].append(log)
        if log["answer"] is None:
            break
    (work / "transcript.json").write_text(json.dumps(messages, indent=1, default=str))
    rec["work"] = str(work)
    return rec


# ---------------------------------------------------------------- main

def load_refs(args):
    def bundle(skill_dir):
        d = Path(skill_dir).expanduser()
        parts = [(d / "SKILL.md").read_text()]
        if (d / "style-reference.md").exists():
            parts.append("# Style reference\n\n" + (d / "style-reference.md").read_text())
        return "\n\n".join(parts)
    def anofox(*flags):
        return subprocess.run(["node", str(HERE / "anofox-reference.mjs"), "--pkg", args.pkg, *flags],
                              capture_output=True, text=True, check=True).stdout
    # only the arms asked for (#1074: a CI runner has no ~/.claude skills)
    make = {"anofox": lambda: anofox(), "anofox-lean": lambda: anofox("--lean"),
            "python": lambda: bundle(args.python_ref), "vegalite": lambda: bundle(args.vega_ref)}
    arms = [a.strip() for a in args.arms.split(",")]
    return {a: make[a]() for a in arms if a in make}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", default="pilot", help="pilot | all (the 20 comparison tasks) | coverage (the gap-finding set) | id,id,…")
    ap.add_argument("--family", help="with --tasks coverage: only these families (time,map,kpi,…)")
    ap.add_argument("--arms", default="anofox,python,vegalite")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--model", default="claude-sonnet-5")
    ap.add_argument("--effort", default="medium")
    ap.add_argument("--data", required=True, help="folder of <dataset>.duckdb (make-data.py)")
    ap.add_argument("--dashboard", default=str(ROOT / "target/release/dashboard"))
    ap.add_argument("--python", default=sys.executable, help="python with pandas/matplotlib/seaborn/vl-convert")
    ap.add_argument("--pkg", default=str(ROOT / "web/pkg"))
    ap.add_argument("--boundaries", help="the optional map layers for the anofox CLI (#1057), e.g. web/data/boundaries")
    ap.add_argument("--python-ref", default="~/.claude/skills/matplotlib")
    ap.add_argument("--vega-ref", default="~/.claude/skills/altair")
    ap.add_argument("--out", required=True)
    ap.add_argument("--dry", action="store_true", help="scripted turns, no API")
    ap.add_argument("--yes", action="store_true", help="don't ask before spending")
    a = ap.parse_args()

    tasks = [json.loads(l) for f in ("tasks.jsonl", "coverage.jsonl")
             for l in (HERE / f).read_text().splitlines() if l.strip()]
    if a.tasks == "pilot":
        tasks = [t for t in tasks if t["pilot"]]
    elif a.tasks == "all":
        tasks = [t for t in tasks if "family" not in t]
    elif a.tasks == "coverage":
        tasks = [t for t in tasks if "family" in t]
        if a.family:
            tasks = [t for t in tasks if t["family"] in a.family.split(",")]
    else:
        want = set(a.tasks.split(","))
        tasks = [t for t in tasks if t["id"] in want]
    arms = a.arms.split(",")
    if a.dry:
        tasks = [t for t in tasks if t["dataset"] == "sales"][:1]
    out = Path(a.out)
    (out / "charts").mkdir(parents=True, exist_ok=True)
    cfg = {"data": str(Path(a.data).resolve()), "dashboard": str(Path(a.dashboard).resolve()), "python": a.python,
           "boundaries": str(Path(a.boundaries).resolve()) if a.boundaries else None,
           "workdir": str((out / "work").resolve())}
    Path(cfg["workdir"]).mkdir(exist_ok=True)
    refs = load_refs(a)

    meta = {"model": a.model, "effort": a.effort, "reps": a.reps, "arms": arms, "tasks": [t["id"] for t in tasks],
            "dry": a.dry, "started": time.strftime("%Y-%m-%dT%H:%M:%S"),
            "ref_sha": {k: hashlib.sha256(v.encode()).hexdigest()[:12] for k, v in refs.items() if k in arms},
            "ref_chars": {k: len(v) for k, v in refs.items() if k in arms}}
    if not a.dry:
        key = os.environ.get("ANTHROPIC_API_KEY")
        if not key:
            sys.exit("ANTHROPIC_API_KEY is not set")
        api = Anthropic(key, a.model, a.effort)
        meta["prefix_tokens"] = {}
        for arm in arms:
            b = base(arm)
            sysblocks = [{"type": "text", "text": INSTRUCTIONS.format(chart_tool=CHART_TOOLS[b]["name"]) + "\n" + ARM_NOTES[b]},
                         {"type": "text", "text": refs[arm]}]
            meta["prefix_tokens"][arm] = api.count(sysblocks, COMMON_TOOLS + [CHART_TOOLS[b]])
        n = len(tasks) * len(arms) * a.reps
        print(f"{n} runs ({len(tasks)} tasks × {len(arms)} arms × {a.reps}), prefix tokens {meta['prefix_tokens']}")
        if not a.yes and input("Spend on these runs? [y/N] ").strip().lower() != "y":
            sys.exit("stopped")
    (out / "meta.json").write_text(json.dumps(meta, indent=1))

    # interleave arms per task and rep, so drift over time hits every arm alike
    with open(out / "runs.jsonl", "a") as f:
        for rep in range(1, a.reps + 1):
            for t in tasks:
                for arm in arms:
                    model = Scripted(base(arm)) if a.dry else Anthropic(os.environ["ANTHROPIC_API_KEY"], a.model, a.effort)
                    rec = run_one(t, arm, rep, model, cfg, out, refs)
                    f.write(json.dumps(rec, default=str) + "\n")
                    f.flush()
                    p = rec["phases"]
                    print(f"{rec['run']}: " + "; ".join(
                        f"{x['phase']} {'answered' if x['answer'] else 'NO ANSWER'} {len(x['calls'])} calls ${x['cost']:.3f}" for x in p), flush=True)


if __name__ == "__main__":
    main()
