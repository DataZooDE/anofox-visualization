# How many duplicate customers do we have, and which should we merge?

A use-case dashboard for [anofox-tabular](https://github.com/DataZooDE/anofox-tabular):
duplicate / entity matching. A made-up garden-supply retailer, Grünwerk,
moves the customers of its webshop, ERP and loyalty cards into one CRM. The
CRM owner has to decide how strict the matching is: a low threshold merges
different people (above all, members of one household), a high one leaves
duplicates — double mailings and split purchase histories. Pairs between the
match threshold and the auto-merge threshold go to a steward. It's the
"Which duplicate customers should we merge?" example of the
[live demo](https://datazoode.github.io/anofox-visualization/) (sidebar:
anofox extensions → Tabular).

## Stored results, not live

anofox_tabular has **no DuckDB-Wasm build**, so the browser demo can't run it.
The computation runs natively and the dashboard reads its results:

| File | Role |
|---|---|
| `data.sql` | 1,594 synthetic customers (deterministic; 22 % businesses with a valid German VAT ID, 94 who share a household with another customer) and their 2,502 records in three systems, each with its own conventions: the ERP writes surnames in capitals, abbreviates "Str." and formats phones `030/23456789` and VAT IDs `DE 123 456 789`; the loyalty cards write umlauts out and carry an email on half the cards; the webshop has second accounts. Plus typos, old addresses and a few malformed emails. |
| `compute.sql` | Normalises the match keys with the extension (`phonenumber_format` to E.164, `vat_normalize` + `vat_is_valid`, `email_is_valid`), measures exact duplicates per key before and after with one `run_checks` suite of `duplicate_count` checks, and key fill / validity per system (`phonenumber_is_valid`, `vat_is_valid`, `email_is_valid`). Candidate pairs come from blocking on the keys, the name + city and the postcode + surname prefix; each pair gets a weighted similarity score (DuckDB's `jaro_winkler_similarity` for names and streets, equality of the normalised keys). Match clusters (connected components) are stored for thresholds 50, 55, …, 95 %. |
| `../../scripts/gen-tabular-cases.sh dedup` | Runs `compute.sql` natively and writes the tables to `web/data/tabular-dedup/*.csv`, plus `provenance.csv`. |
| `showcase.sql` | A link to `web/examples/tabular-dedup.sql`, the single source of the dashboard. It only reads the CSVs, and says on the page that the results are stored. |

The truth label of each pair (same customer / same household / different
customers) is known because the data is made up; in a real project it is
the steward-reviewed sample the threshold is tuned on.

Regenerate (DuckDB v1.5.6, from the repository root):

```sh
duckdb -c "INSTALL anofox_tabular FROM community"   # once (signed; FORCE INSTALL replaces a get.erpl.io install)
DUCKDB=duckdb scripts/gen-tabular-cases.sh dedup
cd web && ../target/release/dashboard --check --strict ../examples/tabular-dedup/showcase.sql
```

## The dashboard

It opens with the question and who asks it, then the inputs and a KPI strip
at the chosen threshold: customers after merging (vs. records), precision and
recall (vs. a plain `GROUP BY email`), pairs merged automatically, pairs for
review and household pairs that would be merged wrongly (green at none,
amber up to five, red beyond; precision and recall amber below 90 %). The inputs only
re-cut the stored pair scores:

- **Match threshold (%)** — pairs at or above it are matches; the clusters use
  the nearest stored 5-point step;
- **Merge without review from (%)** — the auto-merge threshold; the pairs in
  between form the review list;
- **Systems** — only pairs whose two records come from the selected systems.

1. **Overview** — candidate pairs by score and label (households fill the gap
   between duplicates and different customers), matched pairs by system pair
   and merged customers by number of records.
2. **Match quality** — precision and recall by threshold, exact duplicates per
   key before and after normalising, key fill per system and the evidence
   behind the matched pairs.
3. **Clusters** — the largest cluster and the number of customers by
   threshold step (chaining below 75 %), and the 12 largest clusters record
   by record, with the clusters that mix several customers flagged.
4. **Review list** — how many pairs merge automatically and how many a steward
   checks, and the review pairs, the most doubtful first.
