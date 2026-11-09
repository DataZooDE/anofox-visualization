import json, os, sys
import duckdb

FIRST = "(SELECT po_id, line_id, min(received_at) AS first_at FROM po_receipt GROUP BY ALL)"
OPEN = "(SELECT po_id, line_id, sum(received_qty) AS rq FROM po_receipt GROUP BY ALL)"
MX = "(SELECT max(month) FROM m5)"
T = [
 # sales
 ("sales-region-revenue","sales",True,"Which region brings in the most revenue (units × price) over the whole period, and how far ahead of the second region is it?",
  "SELECT region, sum(units*price) AS revenue FROM sales GROUP BY 1 ORDER BY 2 DESC",
  "Split that by channel.","SELECT region, channel, sum(units*price) AS revenue FROM sales GROUP BY ALL ORDER BY 1,2"),
 ("sales-trend","sales",False,"How has total monthly revenue developed? Is it growing?",
  "SELECT month, sum(units*price) AS revenue FROM sales GROUP BY 1 ORDER BY 1",
  "Show one line per region instead.","SELECT month, region, sum(units*price) AS revenue FROM sales GROUP BY ALL ORDER BY 1,2"),
 ("sales-channel-share","sales",False,"What share of all units does each channel have?",
  "SELECT channel, sum(units) AS total_units, round(100*sum(units)/(sum(sum(units)) OVER ()),1) AS share_pct FROM sales GROUP BY 1 ORDER BY 2 DESC",
  "Show the channel shares within each region, as 100 % stacked bars.","SELECT region, channel, round(100*sum(units)/(sum(sum(units)) OVER (PARTITION BY region)),1) AS share_pct FROM sales GROUP BY 1, 2 ORDER BY 1,2"),
 ("sales-season","sales",False,"Which calendar month is strongest on average in units sold?",
  "SELECT month(month) AS m, avg(t) AS avg_units FROM (SELECT month, sum(units) AS t FROM sales GROUP BY 1) GROUP BY 1 ORDER BY 2 DESC",
  "Only for EU.","SELECT month(month) AS m, avg(t) AS avg_units FROM (SELECT month, sum(units) AS t FROM sales WHERE region='EU' GROUP BY 1) GROUP BY 1 ORDER BY 2 DESC"),
 # supply chain
 ("sc-cost","supply_chain",True,"Which ordering policy has the lowest total cost (holding plus backlog cost)?",
  "SELECT policy, sum(cost_holding) AS holding, sum(cost_backlog) AS backlog, sum(cost_holding+cost_backlog) AS total FROM sc_weeks GROUP BY 1 ORDER BY 4",
  "Split it by scenario.","SELECT scenario, policy, sum(cost_holding+cost_backlog) AS total FROM sc_weeks GROUP BY ALL ORDER BY 1,2"),
 ("sc-backlog-stage","supply_chain",False,"Which stage carries the most backlog on average?",
  "SELECT stage_no, stage, avg(backlog) AS avg_backlog FROM sc_weeks GROUP BY ALL ORDER BY 1",
  "Break it down by policy.","SELECT stage_no, stage, policy, avg(backlog) AS avg_backlog FROM sc_weeks GROUP BY ALL ORDER BY 1,3"),
 ("sc-bullwhip","supply_chain",False,"Does the variability of orders (standard deviation of order_out) grow from the retailer up to the supplier?",
  "SELECT stage_no, stage, stddev(order_out) AS sd_orders FROM sc_weeks GROUP BY ALL ORDER BY 1",
  "Compare that for each smoothing setting.","SELECT smoothing, stage_no, stage, stddev(order_out) AS sd_orders FROM sc_weeks GROUP BY ALL ORDER BY 1,2"),
 ("sc-ontime","supply_chain",False,"What share of shipped quantity arrives on time at each stage?",
  "SELECT stage_no, stage, sum(on_time)/sum(shipped) AS on_time_share FROM sc_weeks GROUP BY ALL ORDER BY 1",
  "Per scenario.","SELECT scenario, stage_no, stage, sum(on_time)/sum(shipped) AS on_time_share FROM sc_weeks GROUP BY ALL ORDER BY 1,2"),
 # lead times
 ("lt-late","lead_times",True,"Which supplier delivers late most often (first receipt after the promised date)?",
  f"SELECT s.name, avg((f.first_at > h.promised_at)::INT) AS late_share, count(*) AS lines FROM po_history h JOIN {FIRST} f USING (po_id, line_id) JOIN suppliers s USING (supplier_id) GROUP BY 1 ORDER BY 2 DESC",
  "Show how that supplier's late share developed by order month.",
  f"SELECT s.name, date_trunc('month', h.ordered_at) AS m, avg((f.first_at > h.promised_at)::INT) AS late_share FROM po_history h JOIN {FIRST} f USING (po_id, line_id) JOIN suppliers s USING (supplier_id) GROUP BY ALL ORDER BY 1,2"),
 ("lt-leadtime","lead_times",False,"What is each supplier's average actual lead time (order to first receipt, in days), compared with the days they promise?",
  f"SELECT s.name, s.promised_days, avg(f.first_at - h.ordered_at) AS actual_days FROM po_history h JOIN {FIRST} f USING (po_id, line_id) JOIN suppliers s USING (supplier_id) GROUP BY ALL ORDER BY 3 DESC",
  "Show the spread of lead times per supplier instead of the average.",
  f"SELECT s.name, quantile_cont(f.first_at - h.ordered_at, [0.1,0.25,0.5,0.75,0.9]) AS q FROM po_history h JOIN {FIRST} f USING (po_id, line_id) JOIN suppliers s USING (supplier_id) GROUP BY 1 ORDER BY 1"),
 ("lt-top-skus","lead_times",False,"Which 10 SKUs did we order the most of?",
  "SELECT sku_id, sum(ordered_qty) AS qty FROM po_history GROUP BY 1 ORDER BY 2 DESC LIMIT 10",
  "Show cancelled quantity per SKU instead.","SELECT sku_id, sum(cancelled_qty) AS cancelled FROM po_history GROUP BY 1 ORDER BY 2 DESC"),
 ("lt-open","lead_times",False,"How much ordered quantity is still open (ordered minus cancelled minus received) per supplier?",
  f"SELECT s.name, sum(h.ordered_qty) - sum(h.cancelled_qty) - coalesce(sum(r.rq),0) AS open_qty FROM po_history h LEFT JOIN {OPEN} r USING (po_id, line_id) JOIN suppliers s USING (supplier_id) GROUP BY 1 ORDER BY 2 DESC",
  "Only for lines with status 'open'.",
  f"SELECT s.name, sum(h.ordered_qty) - sum(h.cancelled_qty) - coalesce(sum(r.rq),0) AS open_qty FROM po_history h LEFT JOIN {OPEN} r USING (po_id, line_id) JOIN suppliers s USING (supplier_id) WHERE h.status='open' GROUP BY 1 ORDER BY 2 DESC"),
 # claims
 ("cl-fraud-lob","claims",True,"Which line of business has the highest fraud rate?",
  "SELECT lob, avg(fraud::INT) AS fraud_rate, count(*) AS claims FROM claims GROUP BY 1 ORDER BY 2 DESC",
  "And how does it differ between injury and non-injury claims?","SELECT lob, injury, avg(fraud::INT) AS fraud_rate, count(*) AS claims FROM claims GROUP BY ALL ORDER BY 1,2"),
 ("cl-amount","claims",False,"How are claim amounts distributed, and are fraudulent claims larger?",
  "SELECT fraud, count(*) AS n, median(amount_eur) AS median_eur, avg(amount_eur) AS mean_eur FROM claims GROUP BY 1",
  "Per line of business.","SELECT lob, fraud, median(amount_eur) AS median_eur, avg(amount_eur) AS mean_eur FROM claims GROUP BY ALL ORDER BY 1,2"),
 ("cl-weekly","claims",False,"How many claims come in per week, and is there a trend?",
  "SELECT week, count(*) AS claims FROM claims GROUP BY 1 ORDER BY 1",
  "Show fraudulent claims as a separate series.","SELECT week, fraud, count(*) AS claims FROM claims GROUP BY ALL ORDER BY 1,2"),
 ("cl-late-report","claims",False,"Are late-reported claims more often fraudulent than claims reported on time?",
  "SELECT late_report, avg(fraud::INT) AS fraud_rate, count(*) AS n FROM claims GROUP BY 1",
  "Compare it for new policies versus older ones too.","SELECT new_policy, late_report, avg(fraud::INT) AS fraud_rate, count(*) AS n FROM claims GROUP BY ALL ORDER BY 1,2"),
 ("cl-score","claims",False,"Does the suspicion score p_suspicious separate fraudulent from legitimate claims?",
  "SELECT fraud, avg(p_suspicious) AS mean_score, quantile_cont(p_suspicious, [0.25,0.5,0.75]) AS q FROM claims GROUP BY 1",
  "Show the fraud rate by p_suspicious decile.","SELECT decile, avg(fraud::INT) AS fraud_rate FROM (SELECT fraud, ntile(10) OVER (ORDER BY p_suspicious) AS decile FROM claims) GROUP BY 1 ORDER BY 1"),
 # m5
 ("m5-top-items","m5",True,"Which 5 items sold the most units in total?",
  "SELECT item, sum(units) AS total_units FROM m5 GROUP BY 1 ORDER BY 2 DESC LIMIT 5",
  "Show their monthly trend.","SELECT item, month, sum(units) AS total_units FROM m5 WHERE item IN (SELECT item FROM m5 GROUP BY 1 ORDER BY sum(units) DESC LIMIT 5) GROUP BY ALL ORDER BY 1,2"),
 ("m5-stores","m5",False,"How do the stores compare in total units sold?",
  "SELECT store, sum(units) AS total_units FROM m5 GROUP BY 1 ORDER BY 2 DESC",
  "Show the monthly trend per store.","SELECT store, month, sum(units) AS total_units FROM m5 GROUP BY ALL ORDER BY 1,2"),
 ("m5-yoy","m5",False,"How did total units in the last 12 months compare with the 12 months before?",
  f"SELECT sum(units) FILTER (WHERE month > {MX} - INTERVAL 12 MONTH) AS last_12, sum(units) FILTER (WHERE month <= {MX} - INTERVAL 12 MONTH AND month > {MX} - INTERVAL 24 MONTH) AS prior_12 FROM m5",
  "Per store.",
  f"SELECT store, sum(units) FILTER (WHERE month > {MX} - INTERVAL 12 MONTH) AS last_12, sum(units) FILTER (WHERE month <= {MX} - INTERVAL 12 MONTH AND month > {MX} - INTERVAL 24 MONTH) AS prior_12 FROM m5 GROUP BY 1 ORDER BY 1"),
]

data, out = sys.argv[1], sys.argv[2]
with open(out, "w") as f:
    for (i, ds, pilot, q, t, fu, ft) in T:
        c = duckdb.connect(os.path.join(data, f"{ds}.duckdb"), read_only=True)
        for s in (t, ft):
            try:
                assert c.execute(s).fetchall(), f"{i}: no rows"
            except Exception as e:
                sys.exit(f"{i}: {e}")
        f.write(json.dumps({"id": i, "dataset": ds, "pilot": pilot, "question": q, "truth_sql": t,
                            "followup": fu, "followup_truth_sql": ft}) + "\n")
print(len(T), "tasks,", sum(p for _, _, p, *_ in T), "pilot")
