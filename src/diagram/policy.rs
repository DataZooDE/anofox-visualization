//! A decision's policy in an influence diagram (#1279): the `policy` of a
//! node's `::BELIEF` JSON (the `bayes-net` plugin's `policy :=`), drawn as a
//! table in the decision's box — a row per configuration of its
//! informational parents, a column per option with its expected utility,
//! the chosen option marked, the configuration the evidence picks on a band.

use crate::format::{format_number_l, Locale};
use crate::MetricFmt;
use ggplot_rs::graph::NodeTable;
use serde_json::Value as J;

/// One configuration: the parents' states, the expected utility per option,
/// the chosen option.
#[derive(Clone, Debug, PartialEq)]
pub struct PolicyRow {
    pub parents: Vec<String>,
    pub eu: Vec<Option<f64>>,
    pub chosen: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    /// The informational parents' labels.
    pub parents: Vec<String>,
    pub options: Vec<String>,
    pub rows: Vec<PolicyRow>,
    /// The configuration the evidence picks.
    pub current: Option<usize>,
    /// The expected utilities as the table shows them (the locale's).
    cells: Vec<Vec<String>>,
}

fn texts(v: Option<&J>) -> Vec<String> {
    v.and_then(J::as_array)
        .map(|a| {
            a.iter()
                .map(|x| match x {
                    J::String(s) => s.clone(),
                    J::Null => String::new(),
                    o => o.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Policy {
    /// `{parents, options, rows: [{parents, eu, chosen}], current}`; `None`
    /// without options or rows.
    pub fn parse(v: &J) -> Option<Policy> {
        let o = v.as_object()?;
        let options = texts(o.get("options"));
        let rows: Vec<PolicyRow> = o
            .get("rows")?
            .as_array()?
            .iter()
            .filter_map(|r| {
                let r = r.as_object()?;
                Some(PolicyRow {
                    parents: texts(r.get("parents")),
                    eu: r
                        .get("eu")
                        .and_then(J::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|x| x.as_f64().filter(|f| f.is_finite()))
                                .collect()
                        })
                        .unwrap_or_default(),
                    chosen: r
                        .get("chosen")
                        .and_then(J::as_u64)
                        .map(|k| k as usize)
                        .filter(|k| *k < options.len()),
                })
            })
            .collect();
        if options.is_empty() || rows.is_empty() {
            return None;
        }
        let current = o
            .get("current")
            .and_then(J::as_u64)
            .map(|k| k as usize)
            .filter(|k| *k < rows.len());
        let mut p = Policy {
            parents: texts(o.get("parents")),
            options,
            rows,
            current,
            cells: Vec::new(),
        };
        p.localize(&Locale::default());
        Some(p)
    }

    /// Format the expected utilities for `locale`.
    pub fn localize(&mut self, locale: &Locale) {
        self.cells = self
            .rows
            .iter()
            .map(|r| {
                (0..self.options.len())
                    .map(|k| {
                        r.eu.get(k)
                            .copied()
                            .flatten()
                            .map(|v| format_number_l(v, MetricFmt::Plain, locale))
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect();
    }

    /// A row's name: its parents' states (`—` for none).
    fn name(r: &PolicyRow) -> String {
        if r.parents.is_empty() {
            "expected".into()
        } else {
            r.parents.join(", ")
        }
    }

    /// The table in the decision's box: the parents' labels over the row
    /// names, the options over their expected utilities; a click on an
    /// option's column names that option (`keys`).
    pub fn table(&self) -> NodeTable {
        let mut header = vec![self.parents.join(", ")];
        header.extend(self.options.iter().cloned());
        let mut keys = vec![String::new()];
        keys.extend(self.options.iter().cloned());
        NodeTable {
            header,
            rows: self
                .rows
                .iter()
                .zip(&self.cells)
                .map(|(r, cells)| {
                    let mut row = vec![Self::name(r)];
                    row.extend(cells.iter().cloned());
                    row
                })
                .collect(),
            marked: self.rows.iter().map(|r| r.chosen.map(|k| k + 1)).collect(),
            current: self.current,
            keys,
        }
    }

    /// What the policy says, for the node's hover text and the screen
    /// reader: the choice for the configuration the evidence picks (or the
    /// only one), else the choice per configuration.
    pub fn summary(&self) -> String {
        let pick = |i: usize| {
            let r = &self.rows[i];
            let k = r.chosen?;
            let eu = self.cells[i].get(k).filter(|c| !c.is_empty());
            Some(match eu {
                Some(eu) => format!("{} (expected utility {eu})", self.options[k]),
                None => self.options[k].clone(),
            })
        };
        match (self.current, self.rows.len()) {
            (Some(i), _) => pick(i)
                .map(|c| format!("{c} given {}", self.given(i)))
                .unwrap_or_default(),
            (None, 1) => pick(0).unwrap_or_default(),
            _ => self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(i, r)| {
                    Some(format!("{} if {}", self.options[r.chosen?], self.given(i)))
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// Row `i`'s configuration as text: `Test? = test, Seismic = open`.
    fn given(&self, i: usize) -> String {
        self.rows[i]
            .parents
            .iter()
            .enumerate()
            .map(|(k, s)| match self.parents.get(k) {
                Some(p) if !p.is_empty() => format!("{p} = {s}"),
                _ => s.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn drill() -> Policy {
        Policy::parse(&json!({
            "parents": ["Test?", "Seismic"],
            "options": ["drill", "don't drill"],
            "rows": [
                {"parents": ["no test", "none"], "eu": [20.0, 0.0], "chosen": 0},
                {"parents": ["test", "diffuse"], "eu": [-25.625, -10.0], "chosen": 1},
                {"parents": ["test", "open"], "eu": [1234.5, null], "chosen": 0},
            ],
            "current": 2,
        }))
        .unwrap()
    }

    #[test]
    fn a_policy_becomes_a_table() {
        let t = drill().table();
        assert_eq!(t.header, ["Test?, Seismic", "drill", "don't drill"]);
        assert_eq!(t.rows[1], ["test, diffuse", "-25.63", "-10"]);
        assert_eq!(t.rows[2], ["test, open", "1,234.5", ""]);
        assert_eq!(t.marked, [Some(1), Some(2), Some(1)]);
        assert_eq!(t.current, Some(2));
        assert_eq!(t.keys, ["", "drill", "don't drill"]);
        // the locale's numbers
        let mut p = drill();
        p.localize(&Locale::from_tag("de-DE").unwrap());
        assert_eq!(p.table().rows[2][1], "1.234,5");
        // no options or rows: none
        assert!(Policy::parse(&json!({"options": [], "rows": [{"parents": []}]})).is_none());
        assert!(Policy::parse(&json!({"options": ["a"], "rows": []})).is_none());
    }

    #[test]
    fn a_policy_reads_as_text() {
        assert_eq!(
            drill().summary(),
            "drill (expected utility 1,234.5) given Test? = test, Seismic = open"
        );
        let mut p = drill();
        p.current = None;
        assert_eq!(
            p.summary(),
            "drill if Test? = no test, Seismic = none, don't drill if Test? = test, Seismic = diffuse, drill if Test? = test, Seismic = open"
        );
        let one = Policy::parse(&json!({"parents": [], "options": ["test", "no test"],
            "rows": [{"parents": [], "eu": [12.5, 20], "chosen": 1}]}))
        .unwrap();
        assert_eq!(one.summary(), "no test (expected utility 20)");
        assert_eq!(one.table().rows[0], ["expected", "12.5", "20"]);
        assert_eq!(one.table().header[0], "");
    }

    /// The decision's box holds the policy table; information arcs are
    /// dotted; a utility's expected value is in its diamond; the summary
    /// says what to decide.
    #[test]
    fn an_influence_diagram_draws_its_policy() {
        use crate::{render_with, Column, RenderOptions, Role};
        use ggplot_rs::prelude::Value;
        let s = |v: &str| Value::Str(v.into());
        let v = |x: &[&str]| x.iter().map(|y| s(y)).collect::<Vec<_>>();
        let cols = vec![
            Column::new("id", Role::Diagram, v(&["seismic", "drill", "payoff"])),
            Column::new("l", Role::Label, v(&["Seismic", "Drill?", "Payoff"])),
            Column::new("k", Role::Shape, v(&["chance", "decision", "utility"])),
            Column::new(
                "b",
                Role::Belief,
                vec![
                    s(
                        r#"{"kind": "chance", "states": [{"state": "closed", "p": 0}, {"state": "open", "p": 1}], "evidence": {"state": "open"}}"#,
                    ),
                    s(
                        r#"{"kind": "decision", "states": [{"state": "drill"}, {"state": "don't drill"}],
                         "policy": {"parents": ["Seismic"], "options": ["drill", "don't drill"],
                                    "rows": [{"parents": ["closed"], "eu": [77.5, 0], "chosen": 0},
                                             {"parents": ["open"], "eu": [-12.25, 0], "chosen": 1}],
                                    "current": 1}}"#,
                    ),
                    Value::Na,
                ],
            ),
            Column::new(
                "u",
                Role::Metric(MetricFmt::Plain),
                vec![Value::Na, Value::Na, Value::Float(22.5)],
            ),
            Column::new(
                "e",
                Role::Edges,
                vec![
                    s(
                        r#"[{"from": "seismic", "to": "drill", "style": "dotted"}, {"from": "drill", "to": "payoff"}]"#,
                    ),
                    Value::Na,
                    Value::Na,
                ],
            ),
        ];
        let svg = render_with(&cols, 900, 500, &RenderOptions::default()).unwrap();
        roxmltree::Document::parse(&svg).expect("well-formed");
        for want in [
            "dp-node-table-chosen",
            "dp-node-table-current",
            ">77.5<",
            ">-12.25<",
            "data-state=\"don&#39;t drill\"",
            ">22.5<",
            "decide Drill?: don&#39;t drill (expected utility 0) given Seismic = open",
        ] {
            assert!(svg.contains(want), "{want}: {svg}");
        }
        // only Seismic's two states have bars, the decision none; the
        // dotted arc no flow class
        assert_eq!(svg.matches("dp-node-state-bar").count(), 2);
        assert!(!svg.contains("dp-flow"));
        let arc = svg
            .split("<path")
            .chain(svg.split("<polyline"))
            .find(|e| e.contains("data-from=\"seismic\""))
            .unwrap();
        assert!(arc.contains("stroke-dasharray"), "{arc}");
    }
}
