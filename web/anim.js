// Chart motion over the renderer's SVG: entrance animations (::ANIMATE).
//
// Driven only by what the SVG carries — `data-plot` / `data-xdomain` on the
// plot <svg>, `data-x` / `data-series` on marks, and the panel directives
// src/lib.rs writes on the root (`data-animate`, `data-xline`). Everything
// runs through Web Animations with fill:"backwards", so a finished animation
// leaves no state behind; under prefers-reduced-motion nothing animates and
// the chart is simply in its final state.
//
// `animKit` is self-contained (no imports, no app.js globals): the static HTML
// export embeds its source (`animKit.toString()`) to replay the same motion.
export function animKit(win) {
  const doc = win.document;
  const NS = "http://www.w3.org/2000/svg";
  const EASE = "cubic-bezier(.22,.61,.36,1)";
  const MANY = 500; // above this many marks: one clip reveal, no per-mark stagger
  const still = () => !!(win.matchMedia && win.matchMedia("(prefers-reduced-motion: reduce)").matches);
  const can = (el) => typeof el.animate === "function";
  let uid = 0;

  // The plot rect and x mapping from the inner `<svg data-plot>`.
  function plotOf(svg) {
    const p = svg.matches("[data-plot]") ? svg : svg.querySelector("svg[data-plot]");
    if (!p) return null;
    const [x, y, w, h] = p.getAttribute("data-plot").split(" ").map(Number);
    const xd = (p.getAttribute("data-xdomain") || "").split(" ").map(Number);
    return { el: p, x, y, w, h, xd: xd.length === 2 && xd.every(Number.isFinite) && xd[1] !== xd[0] ? xd : null };
  }

  // Data marks only — never grid, axis or reference rules (2-point polylines).
  // ggplot-rs tags points and bars with data-x, series lines and bands with
  // data-series; a single-series line carries neither, so it is told apart
  // from the grid by having more than two points.
  const npts = (el) => (el.getAttribute("points") || "").trim().split(/\s+/).length;
  const linesOf = (svg) =>
    [...svg.querySelectorAll("polyline")].filter(
      (pl) =>
        pl.classList.contains("dp-flow") ||
        (!pl.hasAttribute("stroke-dasharray") && (pl.hasAttribute("data-series") || npts(pl) > 2))
    );
  const bandsOf = (svg) => [...svg.querySelectorAll("polygon")].filter((pg) => npts(pg) > 2);
  const barsOf = (svg) => [...svg.querySelectorAll("rect[data-x]")];
  const pointsOf = (svg) => [...svg.querySelectorAll("circle[data-x]")];
  const marksOf = (svg) => [...linesOf(svg), ...bandsOf(svg), ...barsOf(svg), ...pointsOf(svg)];

  // Line draw-in: stroke-dashoffset from the full length to 0.
  function drawLine(pl, delay, dur) {
    let len;
    try {
      len = pl.getTotalLength();
    } catch (_) {
      return;
    }
    if (!len || !isFinite(len) || !can(pl)) return;
    pl.animate(
      [
        { strokeDasharray: len, strokeDashoffset: len },
        { strokeDasharray: len, strokeDashoffset: 0 },
      ],
      { duration: dur, delay, easing: EASE, fill: "backwards" }
    );
  }

  // One clipPath rect over the plot whose width follows `stops`
  // ([[offset, width px from the left edge], …]); removed when done. The rect's
  // own width is the final (full) one, so a browser that can't animate a
  // geometry property just shows the chart.
  function clipReveal(svg, els, stops, dur, delay, easing = "linear") {
    const P = plotOf(svg);
    if (!P || !els.length || !can(P.el)) return;
    const id = "dp-clip-" + ++uid;
    const left = P.x - 8;
    const full = P.w + 16;
    const r = doc.createElementNS(NS, "rect");
    r.setAttribute("x", left);
    r.setAttribute("y", -1e5);
    r.setAttribute("height", 2e5);
    r.setAttribute("width", full);
    const cp = doc.createElementNS(NS, "clipPath");
    cp.setAttribute("id", id);
    cp.setAttribute("class", "dp-clip");
    cp.append(r);
    P.el.prepend(cp);
    for (const e of els) e.setAttribute("clip-path", `url(#${id})`);
    const kf = stops.map(([offset, w]) => ({ offset, width: Math.max(0, Math.min(full, w)) + "px" }));
    let a;
    try {
      a = r.animate(kf, { duration: dur, delay, easing, fill: "backwards" });
    } catch (_) {
      a = null;
    }
    const done = () => {
      for (const e of els) if (e.getAttribute("clip-path") === `url(#${id})`) e.removeAttribute("clip-path");
      cp.remove();
    };
    if (a && a.finished) a.finished.then(done, done);
    else done();
  }

  // Bars grow from their baseline, staggered by x level.
  function growBars(svg, bars, total) {
    const levels = [];
    for (const b of bars) {
      const x = b.getAttribute("data-x");
      if (!levels.includes(x)) levels.push(x);
    }
    // Horizontal bars (::FLIP): two x levels sit apart vertically.
    let flip = false;
    const a = bars[0];
    const b = bars.find((r) => r.getAttribute("data-x") !== a.getAttribute("data-x"));
    if (b) {
      const c = (r) => [+r.getAttribute("x") + +r.getAttribute("width") / 2, +r.getAttribute("y") + +r.getAttribute("height") / 2];
      const [ax, ay] = c(a);
      const [bx, by] = c(b);
      flip = Math.abs(by - ay) > Math.abs(bx - ax);
    }
    const step = Math.min(60, (total * 0.5) / Math.max(1, levels.length));
    for (const r of bars) {
      if (!can(r)) continue;
      const neg = +r.getAttribute("data-value") < 0;
      r.style.transformBox = "fill-box";
      r.style.transformOrigin = flip ? (neg ? "100% 50%" : "0 50%") : neg ? "50% 0" : "50% 100%";
      const from = flip ? "scaleX(0)" : "scaleY(0)";
      r.animate([{ transform: from }, { transform: "none" }], {
        duration: total * 0.55,
        delay: levels.indexOf(r.getAttribute("data-x")) * step,
        easing: EASE,
        fill: "backwards",
      });
    }
  }

  // Points pop in left to right.
  function popPoints(points, total) {
    const xs = points.map((p) => +p.getAttribute("cx"));
    const lo = Math.min(...xs);
    const span = Math.max(...xs) - lo || 1;
    points.forEach((p, i) => {
      if (!can(p)) return;
      p.style.transformBox = "fill-box";
      p.style.transformOrigin = "50% 50%";
      p.animate(
        [
          { opacity: 0, transform: "scale(0)" },
          { opacity: 1, transform: "none" },
        ],
        { duration: 260, delay: ((xs[i] - lo) / span) * total * 0.7, easing: EASE, fill: "backwards" }
      );
    });
  }

  // The pixel x of a data x (epoch seconds on a date axis) — or, on a
  // discrete axis, the centre of the marks at that level.
  function xPx(svg, v) {
    const P = plotOf(svg);
    if (!P) return null;
    const n = Number(v);
    if (P.xd && v !== "" && Number.isFinite(n)) return P.x + ((n - P.xd[0]) / (P.xd[1] - P.xd[0])) * P.w;
    const at = [...svg.querySelectorAll("[data-x]")].filter((m) => m.getAttribute("data-x") === String(v));
    if (!at.length) return null;
    const boxes = at.map((m) => m.getBBox());
    return (Math.min(...boxes.map((b) => b.x)) + Math.max(...boxes.map((b) => b.x + b.width))) / 2;
  }

  // Play the entrance `mode` on a panel's root <svg>:
  //   draw   — lines draw in; points, bars and bands are wiped in with them
  //   grow   — bars grow staggered by x, points pop in, lines draw in
  //   reveal — everything up to the ::XLINE ("Today") wipes in, pauses, then
  //            the rest (forecast fan, median, points) grows forward from it
  //   none / fade — nothing here (the panel itself still fades in)
  // Above MANY marks the per-mark stagger becomes one clip reveal.
  function enter(svg, mode) {
    if (!svg || still() || !mode || mode === "none" || mode === "fade") return;
    const P = plotOf(svg);
    const lines = linesOf(svg);
    const flowing = lines.filter((l) => l.classList.contains("dp-flow"));
    const drawn = lines.filter((l) => !l.classList.contains("dp-flow"));
    const bands = bandsOf(svg);
    const bars = barsOf(svg);
    const points = pointsOf(svg);
    const T = 900;
    if (mode === "reveal" && P) {
      const at = xPx(svg, svg.getAttribute("data-xline") ?? "");
      const w0 = at == null ? 0 : at - (P.x - 8);
      const els = [...lines, ...bands, ...bars, ...points];
      // history wipes in (0–40%), holds at Today, then the future grows forward
      clipReveal(svg, els, w0 > 0 ? [[0, 0], [0.4, w0], [0.52, w0], [1, P.w + 16]] : [[0, 0], [1, P.w + 16]], 2400, 0);
      return;
    }
    drawn.forEach((l) => drawLine(l, 0, T));
    const wipe = [...flowing, ...bands];
    if (mode === "grow" && bars.length && bars.length <= MANY) growBars(svg, bars, T);
    else wipe.push(...bars);
    if (mode === "grow" && points.length && points.length <= MANY) popPoints(points, T);
    else wipe.push(...points);
    clipReveal(svg, wipe, [[0, 0], [1, P ? P.w + 16 : 0]], T, 0, EASE);
  }

  // A copy of the dashboard for the static export must not freeze motion
  // state: drop running clips (WAAPI itself is never serialised).
  function strip(root) {
    root.querySelectorAll("clipPath.dp-clip").forEach((c) => c.remove());
    root.querySelectorAll('[clip-path^="url(#dp-clip-"]').forEach((e) => e.removeAttribute("clip-path"));
  }

  return { enter, strip, marksOf, plotOf, xPx, still };
}
