// Chart motion over the renderer's SVG: entrance animations (::ANIMATE),
// one shared emphasis (dimming) state, ::STORY steppers and re-run tweens
// (::TRANSITION).
//
// Driven only by what the SVG carries — `data-plot` / `data-xdomain` on the
// plot <svg>, `data-x` / `data-series` on marks, and the panel directives
// src/lib.rs writes on the root (`data-animate`, `data-xline`, `data-story`,
// `data-transition`).
// Everything runs through Web Animations with fill:"backwards", so a finished
// animation leaves no state behind; under prefers-reduced-motion nothing
// animates and the chart is simply in its final state.
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
  // from the grid by having more than two points. A flowing (::LINESTYLE
  // 'flow') line keeps `stroke-dasharray` set but is still a data mark —
  // it carries `data-linestyle="flow"` (the linetype aesthetic, #318 C2),
  // not a class of its own.
  const npts = (el) => (el.getAttribute("points") || "").trim().split(/\s+/).length;
  const linesOf = (svg) =>
    [...svg.querySelectorAll("polyline")].filter(
      (pl) =>
        pl.getAttribute("data-linestyle") === "flow" ||
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
  // A ::DIAGRAM (root data-diagram) enters stage by stage: each layer's
  // nodes fade and settle in turn, every edge draws from its source once
  // that node is in, then its arrowhead and label appear.
  function enterDiagram(svg) {
    const nodes = [...svg.querySelectorAll("rect[data-node]")];
    const layer = new Map(nodes.map((n) => [n.getAttribute("data-node"), +n.getAttribute("data-layer") || 0]));
    const step = Math.min(260, 1200 / (Math.max(0, ...layer.values()) + 1));
    const NODE = 320;
    const fade = (el, delay, dur, grow) => {
      if (!can(el)) return;
      if (grow) {
        el.style.transformBox = "fill-box";
        el.style.transformOrigin = "center";
      }
      el.animate([{ opacity: 0, transform: grow ? "scale(.9)" : "none" }, { opacity: 1, transform: "none" }], { duration: dur, delay, easing: EASE, fill: "backwards" });
    };
    const of = (id) => [...svg.querySelectorAll("[data-of]")].filter((g) => g.getAttribute("data-of") === id);
    for (const n of nodes) {
      const id = n.getAttribute("data-node");
      for (const el of [n, ...of(id)]) fade(el, layer.get(id) * step, NODE, nodes.length <= MANY);
    }
    for (const pl of svg.querySelectorAll("polyline[data-edge]")) {
      const from = layer.get(pl.getAttribute("data-from")) || 0;
      const to = layer.get(pl.getAttribute("data-to")) || 0;
      const delay = from * step + NODE * 0.6;
      const dur = Math.max(step, Math.abs(to - from) * step);
      if (pl.classList.contains("dp-flow")) fade(pl, delay, dur, false);
      else drawLine(pl, delay, dur);
      for (const el of svg.querySelectorAll("[data-from]:not(polyline)"))
        if (el.getAttribute("data-from") === pl.getAttribute("data-from") && el.getAttribute("data-to") === pl.getAttribute("data-to")) fade(el, delay + dur, 200, false);
    }
  }

  function enter(svg, mode) {
    if (!svg || still() || !mode || mode === "none" || mode === "fade") return;
    if (svg.hasAttribute("data-diagram")) return enterDiagram(svg);
    const P = plotOf(svg);
    const lines = linesOf(svg);
    const flowing = lines.filter((l) => l.getAttribute("data-linestyle") === "flow");
    const drawn = lines.filter((l) => l.getAttribute("data-linestyle") !== "flow");
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

  // ---- emphasis: one opacity per mark from every layer that dims ----------
  // Hover/click selection, brush & named selections, legend focus, a hovered
  // mark's series focus (#213) and story steps each own a layer: `fn(el)` →
  // an opacity below 1 to dim, or null.
  // A mark shows the lowest of them, so clearing one layer never undoes
  // another (keyboard focus overrides all in CSS).
  const dimmable = (svg) => {
    const set = new Set(marksOf(svg));
    // a ::DIAGRAM's box contents (data-of) and arrowheads / edge labels (data-from)
    svg.querySelectorAll("[data-series], .dp-hit, [data-of], [data-from]").forEach((e) => set.add(e));
    return set;
  };
  function emph(svg, layer, fn) {
    if (!svg) return;
    const st = svg._dpEmph || (svg._dpEmph = new Map());
    if (fn) st.set(layer, fn);
    else if (!st.delete(layer)) return;
    for (const el of dimmable(svg)) {
      let o = 1;
      for (const f of st.values()) {
        const v = f(el);
        if (v != null && v < o) o = v;
      }
      el.style.opacity = o < 1 ? String(o) : "";
    }
  }

  // ---- ::STORY: step through captions, lighting one x (or series) --------
  // An ISO date (time) as epoch seconds, read as UTC like the renderer; else NaN.
  const isoSecs = (s) => {
    const m = /^(\d{4}-\d{2}-\d{2})(?:[ T](\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?))?$/.exec(String(s).trim());
    return m ? Date.parse(`${m[1]}T${m[2] || "00:00"}Z`) / 1000 : NaN;
  };
  const EPOCH = /^-?\d{9,11}$/;
  // Same x? Exact text, else equal numbers / dates (a date key "2026-10-12"
  // against a mark's "2026-10-12 00:00:00", or an older SVG's epoch-seconds
  // key "1791763200" against a mark's "2026-10-12").
  function sameX(a, b) {
    if (a === b) return true;
    if (a == null || b == null) return false;
    const n = Number(a);
    const m = Number(b);
    if (a !== "" && b !== "" && Number.isFinite(n) && Number.isFinite(m)) return n === m;
    const s = EPOCH.test(a) ? n : isoSecs(a);
    const t = EPOCH.test(b) ? m : isoSecs(b);
    if (Number.isFinite(s) && s === t) return true;
    const d = Date.parse(a);
    const e = Date.parse(b);
    return Number.isFinite(d) && d === e;
  }
  const svgEl = (tag, attrs) => {
    const e = doc.createElementNS(NS, tag);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    return e;
  };
  // Caption placement works on boxes [left, top, right, bottom] relative to
  // the chart holder: a shape's bounding box, or for a line or band small
  // boxes along its outline (its bounding box would cover the whole plot).
  const CELL = 4;
  const PAD = 4; // clearance around every mark and label
  const nums = (pts) => (pts || "").trim().split(/[\s,]+/).map(Number);
  const SHAPES = "rect,circle,ellipse,line,polyline,polygon,path";
  const shapes = (el) => (el.matches(SHAPES) ? [el] : [...el.querySelectorAll(SHAPES)]);
  function boxesOf(el, hb) {
    const tag = el.tagName.toLowerCase();
    if ((tag === "polyline" || tag === "polygon") && el.getScreenCTM) {
      const m = el.getScreenCTM();
      const v = nums(el.getAttribute("points"));
      const pts = [];
      for (let i = 0; i + 1 < v.length; i += 2) pts.push([m.a * v[i] + m.c * v[i + 1] + m.e - hb.left, m.b * v[i] + m.d * v[i + 1] + m.f - hb.top]);
      if (tag === "polygon" && pts.length) pts.push(pts[0]);
      const out = [];
      for (let i = 0; i < pts.length; i++) {
        const [x, y] = pts[i];
        if (!Number.isFinite(x + y)) continue;
        const [px, py] = i ? pts[i - 1] : pts[i];
        const n = Math.max(1, Math.ceil(Math.hypot(x - px, y - py) / CELL));
        for (let s = i ? 1 : n; s <= n; s++) {
          const qx = px + ((x - px) * s) / n;
          const qy = py + ((y - py) * s) / n;
          out.push([qx - 1, qy - 1, qx + 1, qy + 1]);
        }
      }
      return out;
    }
    const b = el.getBoundingClientRect();
    return b.width || b.height ? [[b.left - hb.left, b.top - hb.top, b.right - hb.left, b.bottom - hb.top]] : [];
  }
  // Build the stepper for a chart whose root <svg> carries data-story
  // ([{k, t}, …]) and data-story-by ("x" | "series"). Step 0 is the overview;
  // each step dims the other marks, shows its caption (an aria-live region)
  // and draws a connector to the lit marks. Returns {show, step}.
  function story(panel, svg, start = 0) {
    let steps;
    try {
      steps = JSON.parse(svg.getAttribute("data-story") || "[]");
    } catch (_) {
      steps = [];
    }
    const holder = svg.parentElement;
    if (!steps.length || !panel || !holder) return null;
    const by = svg.getAttribute("data-story-by") === "series" ? "series" : "x";
    panel.querySelectorAll(".dp-story, .dp-story-cap, .dp-story-link").forEach((e) => e.remove());
    const bar = doc.createElement("div");
    bar.className = "dp-story";
    bar.setAttribute("role", "group");
    bar.setAttribute("aria-label", "Story steps");
    const btn = (cls, text, label) => {
      const b = doc.createElement("button");
      b.type = "button";
      b.className = "dp-story-btn " + cls;
      b.textContent = text;
      if (label) b.setAttribute("aria-label", label);
      bar.append(b);
      return b;
    };
    const prev = btn("dp-story-prev", "‹", "Previous step");
    const play = btn("dp-story-play", "▶ Play", "");
    const next = btn("dp-story-next", "›", "Next step");
    const pos = doc.createElement("span");
    pos.className = "dp-story-pos";
    bar.append(pos);
    play.hidden = still(); // reduced motion: step by hand, no autoplay
    const cap = doc.createElement("div");
    cap.className = "dp-story-cap";
    cap.setAttribute("aria-live", "polite");
    const link = svgEl("svg", { class: "dp-story-link", "aria-hidden": "true" });
    holder.append(link, cap);
    holder.after(bar);

    let k = Math.max(0, Math.min(steps.length, start | 0));
    let timer = null;
    const diagram = svg.hasAttribute("data-diagram");
    const lit = (el) => {
      const st = steps[k - 1];
      // a ::DIAGRAM step lights its node and every edge touching it
      if (diagram) {
        const id = el.getAttribute("data-node") ?? el.getAttribute("data-of");
        if (id != null) return id === st.k;
        const from = el.getAttribute("data-from");
        return from == null ? null : from === st.k || el.getAttribute("data-to") === st.k;
      }
      if (by === "series") {
        const s = el.getAttribute("data-series");
        return s == null ? null : s === st.k;
      }
      return el.hasAttribute("data-x") ? sameX(el.getAttribute("data-x"), st.k) : false;
    };
    // The caption goes to the free spot nearest the lit marks: inside the
    // chart, over no mark and no text (an occupancy grid of CELL px, summed so
    // each spot is one lookup). With no spot free it drops under the chart
    // (.dp-story-cap-out) and the connector runs up to the lit marks.
    const place = () => {
      link.replaceChildren();
      if (!k) return;
      const litEls = new Set([...dimmable(svg)].filter((el) => lit(el) === true).flatMap(shapes));
      let hb = holder.getBoundingClientRect();
      const sb = svg.getBoundingClientRect();
      const litBs = [];
      const others = [];
      // legend swatches are the small rects that are no data mark
      const swatches = [...svg.querySelectorAll("rect:not([data-x])")].filter((e) => {
        const b = e.getBoundingClientRect();
        return b.width < 24 && b.height < 24;
      });
      for (const el of new Set([...marksOf(svg), ...litEls, ...svg.querySelectorAll("text"), ...swatches])) {
        const bs = boxesOf(el, hb);
        if (litEls.has(el)) litBs.push(...bs);
        else others.push(...bs);
      }
      if (!litBs.length || !hb.width) return;
      const l = Math.min(...litBs.map((b) => b[0]));
      const r = Math.max(...litBs.map((b) => b[2]));
      const mx = (l + r) / 2;
      cap.classList.remove("dp-story-cap-out");
      const cw = cap.offsetWidth;
      const ch = cap.offsetHeight;
      const busy = [...others, ...litBs];
      // distance from a caption at (x, y) to the nearest lit mark
      const gap = (x, y) => {
        let g = Infinity;
        for (const [a, b, c, d] of litBs) g = Math.min(g, Math.hypot(Math.max(a - x - cw, 0, x - c), Math.max(b - y - ch, 0, y - d)));
        return g;
      };
      const x0 = Math.max(0, sb.left - hb.left);
      const y0 = Math.max(0, sb.top - hb.top);
      const W = Math.floor((Math.min(hb.width, sb.right - hb.left) - x0) / CELL);
      const H = Math.floor((Math.min(hb.height, sb.bottom - hb.top) - y0) / CELL);
      const cwN = Math.ceil(cw / CELL);
      const chN = Math.ceil(ch / CELL);
      let best = null;
      if (W >= cwN && H >= chN) {
        const sum = new Int32Array((W + 1) * (H + 1));
        for (const [a, b, c, d] of busy) {
          const i0 = Math.max(0, Math.floor((a - PAD - x0) / CELL));
          const i1 = Math.min(W - 1, Math.floor((c + PAD - x0) / CELL));
          const j0 = Math.max(0, Math.floor((b - PAD - y0) / CELL));
          const j1 = Math.min(H - 1, Math.floor((d + PAD - y0) / CELL));
          for (let j = j0; j <= j1; j++) for (let i = i0; i <= i1; i++) sum[(j + 1) * (W + 1) + i + 1] = 1;
        }
        for (let j = 1; j <= H; j++) for (let i = 1; i <= W; i++) sum[j * (W + 1) + i] += sum[(j - 1) * (W + 1) + i] + sum[j * (W + 1) + i - 1] - sum[(j - 1) * (W + 1) + i - 1];
        const at = (i, j) => sum[j * (W + 1) + i];
        for (let j = 0; j + chN <= H; j++)
          for (let i = 0; i + cwN <= W; i++) {
            if (at(i + cwN, j + chN) - at(i, j + chN) - at(i + cwN, j) + at(i, j)) continue;
            const x = x0 + i * CELL;
            const y = y0 + j * CELL;
            // a connector's length away, as centred on the lit marks as fits
            const g = Math.abs(gap(x, y) - 16) + Math.abs(x + cw / 2 - mx) / 4;
            if (!best || g < best.g) best = { x, y, g };
          }
      }
      let from;
      let to;
      if (best) {
        cap.style.left = best.x + "px";
        cap.style.top = best.y + "px";
        // nearest lit point to the caption, and the caption edge facing it
        const cx = best.x + cw / 2;
        const cy = best.y + ch / 2;
        let d = Infinity;
        for (const [a, b, c, e] of litBs) {
          const p = [Math.max(a, Math.min(c, cx)), Math.max(b, Math.min(e, cy))];
          const q = [Math.max(best.x, Math.min(best.x + cw, p[0])), Math.max(best.y, Math.min(best.y + ch, p[1]))];
          const n = Math.hypot(p[0] - q[0], p[1] - q[1]);
          if (n < d) (d = n), (to = p), (from = q);
        }
      } else {
        cap.classList.add("dp-story-cap-out");
        cap.style.left = cap.style.top = "";
        hb = holder.getBoundingClientRect();
        const top = cap.getBoundingClientRect().top - hb.top;
        // a column through the lit marks whose run down to the caption
        // crosses nothing else, the one nearest their middle
        let pick = null;
        for (let x = Math.ceil(l) + 2; x <= r - 2; x += 2) {
          const ys = litBs.filter((b) => b[0] <= x && x <= b[2]).map((b) => b[3]);
          if (!ys.length) continue;
          const y = Math.max(...ys);
          const clear = !others.some((b) => b[0] <= x && x <= b[2] && b[3] > y && b[1] < top);
          const s = (clear ? 0 : 1e6) + Math.abs(x - mx);
          if (!pick || s < pick.s) pick = { x, y, s };
        }
        if (!pick) return;
        from = [pick.x, top];
        to = [pick.x, pick.y];
      }
      // stop 3px short of the marks
      const len = Math.hypot(to[0] - from[0], to[1] - from[1]);
      if (len < 6) return;
      const end = [to[0] - ((to[0] - from[0]) * 3) / len, to[1] - ((to[1] - from[1]) * 3) / len];
      link.setAttribute("viewBox", `0 0 ${hb.width} ${hb.height}`);
      link.append(svgEl("path", { d: `M${from[0]},${from[1]} L${end[0]},${end[1]}`, class: "dp-flow" }));
    };
    const show = (i) => {
      k = (i + steps.length + 1) % (steps.length + 1);
      emph(svg, "story", k ? (el) => (lit(el) === false ? (el.tagName === "polyline" || el.tagName === "polygon" ? 0.35 : 0.2) : null) : null);
      cap.textContent = k ? steps[k - 1].t : "";
      cap.hidden = !k;
      pos.textContent = k ? `Step ${k} of ${steps.length}` : `Overview · ${steps.length} steps`;
      panel.classList.toggle("dp-story-on", !!k);
      place();
    };
    const stop = () => {
      if (timer) win.clearInterval(timer);
      timer = null;
      play.textContent = "▶ Play";
      play.setAttribute("aria-pressed", "false");
    };
    // steps the viewer takes fade; the first show (also after a re-run) is instant
    bar.addEventListener("click", () => svg.classList.add("dp-emph-fade"), true);
    prev.onclick = () => (stop(), show(k - 1));
    next.onclick = () => (stop(), show(k + 1));
    play.onclick = () => {
      if (timer) return stop();
      play.textContent = "❚❚ Pause";
      play.setAttribute("aria-pressed", "true");
      show(k === steps.length ? 1 : k + 1);
      timer = win.setInterval(() => {
        if (!svg.isConnected) return stop(); // the chart was re-rendered or removed
        if (k >= steps.length) return stop(), show(0);
        show(k + 1);
      }, 2600);
    };
    if (win.ResizeObserver) new win.ResizeObserver(() => place()).observe(holder);
    show(k);
    return { show, step: () => k };
  }

  // ---- ::TRANSITION 'tween': marks move to their new values on a re-run ---
  const GEOM = { rect: ["x", "y", "width", "height"], circle: ["cx", "cy", "r"], polyline: ["points"], polygon: ["points"] };
  // Each data mark by (tag, data-x, data-series) — the keyed join between
  // two renders of the same chart (repeats get a #n suffix).
  function keyed(svg) {
    const m = new Map();
    for (const el of marksOf(svg)) {
      const t = el.tagName.toLowerCase();
      let k = `${t}|${el.getAttribute("data-x") ?? ""}|${el.getAttribute("data-series") ?? ""}`;
      for (let n = 1; m.has(k); n++) k = k.replace(/#\d+$/, "") + "#" + n;
      m.set(k, el);
    }
    return m;
  }
  // Remember a chart's marks before it is replaced (null for many marks).
  // A ::DIAGRAM's state for a re-run tween: what each edge, arrowhead and
  // node carries (width, colour, the box contents) and where each node sits.
  function diagramState(svg) {
    const st = new Map();
    const at = (el, ...a) => a.map((k) => el.getAttribute(k));
    for (const e of svg.querySelectorAll("polyline[data-edge]")) st.set("e|" + e.getAttribute("data-edge"), at(e, "stroke-width", "stroke"));
    for (const a of svg.querySelectorAll("polygon[data-from]")) st.set(`a|${a.getAttribute("data-from")}>${a.getAttribute("data-to")}`, at(a, "fill", "points"));
    for (const n of svg.querySelectorAll("rect[data-node]")) st.set("n|" + n.getAttribute("data-node"), at(n, "stroke", "x", "y"));
    for (const g of svg.querySelectorAll("g[data-of]")) st.set("g|" + g.getAttribute("data-of"), [g.innerHTML, g.cloneNode(true)]);
    return st;
  }
  // The same diagram with other numbers (a run picker, a cross-filter): edges
  // change width and colour in place, node borders their colour, and a box
  // whose contents changed cross-fades them. A different set or placement of
  // nodes cross-fades the whole diagram.
  function tweenDiagram(svg, old, dur) {
    const nodes = [...svg.querySelectorAll("rect[data-node]")];
    const same =
      nodes.length === [...old.keys()].filter((k) => k.startsWith("n|")).length &&
      nodes.every((n) => {
        const o = old.get("n|" + n.getAttribute("data-node"));
        return o && o[1] === n.getAttribute("x") && o[2] === n.getAttribute("y");
      });
    const anim = (el, from, to) => can(el) && el.animate([from, to], { duration: dur, easing: EASE, fill: "backwards" });
    if (!same) return anim(svg, { opacity: 0.35 }, { opacity: 1 });
    for (const e of svg.querySelectorAll("polyline[data-edge]")) {
      const o = old.get("e|" + e.getAttribute("data-edge"));
      if (!o) anim(e, { opacity: 0 }, { opacity: 1 });
      else if (o[0] !== e.getAttribute("stroke-width") || o[1] !== e.getAttribute("stroke"))
        anim(e, { strokeWidth: o[0], stroke: o[1] }, { strokeWidth: e.getAttribute("stroke-width"), stroke: e.getAttribute("stroke") });
    }
    for (const a of svg.querySelectorAll("polygon[data-from]")) {
      const o = old.get(`a|${a.getAttribute("data-from")}>${a.getAttribute("data-to")}`);
      if (!o || o[1] !== a.getAttribute("points")) anim(a, { opacity: 0 }, { opacity: 1 });
      else if (o[0] !== a.getAttribute("fill")) anim(a, { fill: o[0] }, { fill: a.getAttribute("fill") });
    }
    for (const n of nodes) {
      const o = old.get("n|" + n.getAttribute("data-node"));
      if (o[0] !== n.getAttribute("stroke")) anim(n, { stroke: o[0] }, { stroke: n.getAttribute("stroke") });
    }
    for (const g of svg.querySelectorAll("g[data-of]")) {
      const o = old.get("g|" + g.getAttribute("data-of"));
      if (!o || o[0] === g.innerHTML) continue;
      const ghost = o[1];
      ghost.classList.add("dp-ghost");
      g.after(ghost);
      const out = can(ghost) && ghost.animate([{ opacity: 1 }, { opacity: 0 }], { duration: dur, easing: EASE, fill: "forwards" });
      if (out && out.finished) out.finished.then(() => ghost.remove(), () => ghost.remove());
      else ghost.remove();
      anim(g, { opacity: 0 }, { opacity: 1 });
    }
  }

  function geom(svg) {
    if (!svg) return null;
    if (svg.hasAttribute("data-diagram")) return { diagram: diagramState(svg) };
    const m = keyed(svg);
    if (m.size > MANY) return { many: true };
    const out = new Map();
    for (const [k, el] of m) out.set(k, { clone: el.cloneNode(true), attrs: (GEOM[el.tagName.toLowerCase()] || []).map((a) => el.getAttribute(a)) });
    return { marks: out };
  }
  // Move the new chart's marks from where `old` had them: bars and points
  // tween x/y/size, lines and bands their points when the counts match (else
  // cross-fade); new marks fade in, gone ones fade out. Above MANY marks the
  // whole chart cross-fades.
  function tween(svg, old, dur = 300) {
    if (!svg || !old || still()) return;
    if (old.diagram) return svg.hasAttribute("data-diagram") ? tweenDiagram(svg, old.diagram, dur * 2) : undefined;
    const fresh = keyed(svg);
    if (old.many || fresh.size > MANY) {
      if (can(svg)) svg.animate([{ opacity: 0.35 }, { opacity: 1 }], { duration: dur, easing: EASE });
      return;
    }
    const P = plotOf(svg);
    const jobs = [];
    const fade = (el, from, to) => can(el) && el.animate([{ opacity: from }, { opacity: to }], { duration: dur, easing: EASE, fill: "backwards" });
    const ghost = (clone) => {
      clone.classList.add("dp-ghost");
      clone.removeAttribute("tabindex");
      (P ? P.el : svg).append(clone);
      const a = can(clone) && clone.animate([{ opacity: 1 }, { opacity: 0 }], { duration: dur, easing: EASE, fill: "forwards" });
      if (a && a.finished) a.finished.then(() => clone.remove(), () => clone.remove());
      else clone.remove();
    };
    for (const [k, el] of fresh) {
      const o = old.marks.get(k);
      if (!o) {
        fade(el, 0, 1); // enters
        continue;
      }
      old.marks.delete(k);
      const names = GEOM[el.tagName.toLowerCase()] || [];
      const to = names.map((a) => el.getAttribute(a));
      if (names[0] === "points") {
        const a = nums(o.attrs[0]);
        const b = nums(to[0]);
        if (a.length === b.length && a.every(Number.isFinite) && b.every(Number.isFinite)) jobs.push({ el, names, pts: [a, b], to });
        else ghost(o.clone), fade(el, 0, 1); // reshaped: cross-fade
        continue;
      }
      const a = o.attrs.map(Number);
      const b = to.map(Number);
      if (a.every(Number.isFinite) && b.every(Number.isFinite)) jobs.push({ el, names, from: a, b, to });
    }
    for (const o of old.marks.values()) ghost(o.clone); // leaves
    play(jobs, dur);
  }
  // Move each job's mark from its `from` geometry to `to` (attribute values)
  // over `dur` ms, ease-out; the last frame sets the exact end state.
  function play(jobs, dur) {
    if (!jobs.length) return;
    const set = (k) => {
      for (const j of jobs) {
        if (j.pts) {
          const [a, b] = j.pts;
          let p = "";
          for (let i = 0; i < a.length; i += 2) p += (i ? " " : "") + (a[i] + (b[i] - a[i]) * k).toFixed(2) + "," + (a[i + 1] + (b[i + 1] - a[i + 1]) * k).toFixed(2);
          j.el.setAttribute("points", p);
        } else j.names.forEach((n, i) => j.el.setAttribute(n, (j.from[i] + (j.b[i] - j.from[i]) * k).toFixed(2)));
      }
    };
    set(0);
    const t0 = win.performance.now();
    const frame = (t) => {
      const u = Math.max(0, Math.min(1, (t - t0) / dur)); // a frame's time can precede t0
      if (u < 1) {
        set(1 - Math.pow(1 - u, 3));
        win.requestAnimationFrame(frame);
      } else for (const j of jobs) j.names.forEach((n, i) => j.el.setAttribute(n, j.to[i])); // exact end state
    };
    win.requestAnimationFrame(frame);
  }

  // ---- drill (::DRILL): a bar chart zooms between its levels ---------------
  // Down: the clicked bar (data-x = `key`) splits into the next level's bars,
  // which spread out to their places while the other bars fade. Up: the
  // children gather into the bar of the value they belonged to (`key`) and
  // the level's other bars fade in. Bars run along x, or along y when flipped.
  // Anything but plain bars (or a missing key) tweens instead.
  function zoom(svg, old, key, down, dur = 380) {
    if (!svg || !old || still()) return;
    const fresh = [...keyed(svg).values()];
    const was = old.many ? [] : [...old.marks.values()];
    const rect = (el) => el.tagName.toLowerCase() === "rect";
    const box = (el) => GEOM.rect.map((a) => Number(el.getAttribute(a)));
    const ok = (b) => b.every(Number.isFinite);
    if (!fresh.length || !was.length || !fresh.every(rect) || !was.every((o) => rect(o.clone))) return tween(svg, old, dur);
    // Flipped bars share their left edge: the category axis is y.
    const lefts = new Set(was.map((o) => o.attrs[0]));
    const flip = was.length > 1 && lefts.size === 1;
    const along = flip ? 1 : 0;
    const order = (els, b) => els.slice().sort((p, q) => b(p)[along] - b(q)[along]);
    // `n` equal slices of box `b` along the category axis.
    const slices = (b, n) =>
      Array.from({ length: n }, (_, i) => (flip ? [b[0], b[1] + (i * b[3]) / n, b[2], b[3] / n] : [b[0] + (i * b[2]) / n, b[1], b[2] / n, b[3]]));
    // The old bars stay under the new ones while they go.
    const ghostOf = (clone) => {
      clone.classList.add("dp-ghost");
      clone.removeAttribute("tabindex");
      fresh[0].before(clone);
      return clone;
    };
    const fadeOut = (el, d) => {
      const a = can(el) && el.animate([{ opacity: 1 }, { opacity: 0 }], { duration: d, easing: EASE, fill: "forwards" });
      if (a && a.finished) a.finished.then(() => el.remove(), () => el.remove());
      else el.remove();
    };
    const jobs = [];
    const job = (el, from, to) => jobs.push({ el, names: GEOM.rect, from, b: to, to: to.map((v) => v.toFixed(2)) });
    if (down) {
      const src = was.find((o) => o.clone.getAttribute("data-x") === key);
      const s = src && box(src.clone);
      if (!s || !ok(s)) return tween(svg, old, dur);
      // The new level's bars start as slices of the clicked bar.
      const kids = order(fresh, box);
      const from = slices(s, kids.length);
      kids.forEach((el, i) => job(el, from[i], box(el)));
      // (the slices cover the clicked bar exactly: it can go at once)
      for (const o of was) if (o !== src) fadeOut(ghostOf(o.clone), dur / 3);
    } else {
      const dst = fresh.find((el) => el.getAttribute("data-x") === key);
      if (!dst) return tween(svg, old, dur);
      // The bar we came back to grows out of the children's span ...
      const bs = was.map((o) => box(o.clone)).filter(ok);
      if (!bs.length) return tween(svg, old, dur);
      const x0 = Math.min(...bs.map((b) => b[0])), y0 = Math.min(...bs.map((b) => b[1]));
      const x1 = Math.max(...bs.map((b) => b[0] + b[2])), y1 = Math.max(...bs.map((b) => b[1] + b[3]));
      const span = [x0, y0, x1 - x0, y1 - y0];
      const d = box(dst);
      job(dst, span, d);
      // ... while the children gather into it and fade.
      const kids = order(was.map((o) => o.clone), box);
      const into = slices(d, kids.length);
      kids.forEach((c, i) => {
        const g = ghostOf(c);
        job(g, box(g), into[i]);
        fadeOut(g, dur);
      });
      for (const el of fresh) if (el !== dst && can(el)) el.animate([{ opacity: 0 }, { opacity: 1 }], { duration: dur, delay: dur / 3, easing: EASE, fill: "backwards" });
    }
    play(jobs, dur);
  }

  // A copy of the dashboard for the static export must not freeze motion
  // state: drop running clips (WAAPI itself is never serialised), story
  // controls and overlays, and every dimmed mark — the snapshot script
  // rebuilds the stepper at its overview.
  function strip(root) {
    root.querySelectorAll("clipPath.dp-clip").forEach((c) => c.remove());
    root.querySelectorAll('[clip-path^="url(#dp-clip-"]').forEach((e) => e.removeAttribute("clip-path"));
    root.querySelectorAll(".dp-story, .dp-story-cap, .dp-story-link, .dp-ghost").forEach((e) => e.remove());
    root.querySelectorAll(".dp-story-on").forEach((e) => e.classList.remove("dp-story-on"));
    root.querySelectorAll(".dp-emph-fade").forEach((e) => e.classList.remove("dp-emph-fade"));
    root.querySelectorAll(".panel svg [style*='opacity']").forEach((e) => (e.style.opacity = ""));
  }

  return { enter, strip, emph, story, geom, tween, zoom, marksOf, plotOf, xPx, still, sameX };
}
