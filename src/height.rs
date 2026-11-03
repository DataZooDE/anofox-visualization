//! `::HEIGHT` values (#940): pixels (`200`), an aspect ratio of the panel's
//! own width (`'16:9'`, `'0.5'` — height ÷ width), or a share of the window
//! height (`'40vh'`, the browser's; static outputs use
//! [`REFERENCE_WINDOW_H`]).

/// The window height a static output (SVG, PDF, exports) takes for `vh`.
pub const REFERENCE_WINDOW_H: f64 = 900.0;
/// Below this a plain number is a ratio (`0.5`), from it on pixels (`200`).
const RATIO_BELOW: f64 = 10.0;
const MIN_RATIO: f64 = 0.05;
const MAX_RATIO: f64 = 4.0;
pub const MAX_PX: u32 = 5000;
/// The smallest height any form resolves to.
pub const MIN_PX: u32 = 40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PanelHeight {
    Px(u32),
    /// height ÷ width
    Ratio(f64),
    /// a share (0–1] of the window height
    Window(f64),
}

impl PanelHeight {
    /// `None`: not a height (the lint says why: `layout/bad-height`).
    pub fn parse(spec: &str) -> Option<PanelHeight> {
        let s = spec.trim().to_ascii_lowercase();
        let num = |t: &str| t.trim().parse::<f64>().ok().filter(|n| n.is_finite());
        if let Some(v) = s.strip_suffix("vh") {
            let n = num(v)?;
            return (n > 0.0 && n <= 100.0).then_some(PanelHeight::Window(n / 100.0));
        }
        if let Some((w, h)) = s.split_once(':') {
            let (w, h) = (num(w)?, num(h)?);
            let r = h / w;
            return (w > 0.0 && h > 0.0 && (MIN_RATIO..=MAX_RATIO).contains(&r))
                .then_some(PanelHeight::Ratio(r));
        }
        let n = num(&s)?;
        if n <= 0.0 {
            return None;
        }
        if n < RATIO_BELOW {
            return (MIN_RATIO..=MAX_RATIO)
                .contains(&n)
                .then_some(PanelHeight::Ratio(n));
        }
        Some(PanelHeight::Px((n as u32).min(MAX_PX)))
    }

    /// In pixels, for a panel `width` wide in a window `window_h` high.
    pub fn px(self, width: f64, window_h: f64) -> u32 {
        let h = match self {
            PanelHeight::Px(p) => return p.max(1),
            PanelHeight::Ratio(r) => width * r,
            PanelHeight::Window(s) => window_h * s,
        };
        (h.round() as u32).clamp(MIN_PX, MAX_PX)
    }

    /// For the browser: `{"px": n}`, `{"ratio": r}` or `{"vh": share}`.
    pub fn to_json(self) -> serde_json::Value {
        match self {
            PanelHeight::Px(p) => serde_json::json!({ "px": p }),
            PanelHeight::Ratio(r) => serde_json::json!({ "ratio": r }),
            PanelHeight::Window(s) => serde_json::json!({ "vh": s }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PanelHeight::*;
    use super::*;

    #[test]
    fn every_form_parses_and_resolves() {
        assert_eq!(PanelHeight::parse("200"), Some(Px(200)));
        assert_eq!(PanelHeight::parse(" 320.7 "), Some(Px(320)));
        assert_eq!(PanelHeight::parse("16:9"), Some(Ratio(9.0 / 16.0)));
        assert_eq!(PanelHeight::parse("0.5"), Some(Ratio(0.5)));
        assert_eq!(PanelHeight::parse("40vh"), Some(Window(0.4)));
        assert_eq!(PanelHeight::parse("40VH"), Some(Window(0.4)));
        for bad in [
            "", "0", "-3", "abc", "16:0", "0:9", "1:100", "0vh", "120vh", "0.01", "nan",
        ] {
            assert_eq!(PanelHeight::parse(bad), None, "{bad}");
        }
        assert_eq!(Ratio(0.5).px(800.0, 900.0), 400);
        assert_eq!(Window(0.4).px(800.0, REFERENCE_WINDOW_H), 360);
        assert_eq!(Ratio(0.05).px(100.0, 900.0), MIN_PX);
        assert_eq!(Px(9999).px(1.0, 1.0), 9999);
        assert_eq!(PanelHeight::parse("99999"), Some(Px(MAX_PX)));
    }
}
