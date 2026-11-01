// A React wrapper around <anofox-dashboard> (#771). No build step and no
// React import of its own: pass your React in.
//
//   import React from "react";
//   import "https://dash.example.com/sdk/anofox-embed.js";
//   import { makeAnofoxDashboard } from "https://dash.example.com/sdk/anofox-react.js";
//   const AnofoxDashboard = makeAnofoxDashboard(React);
//
//   <AnofoxDashboard src="https://dash.example.com/d/sales" getToken={fetchToken}
//     vars={{ region }} onState={(vars) => …} onLink={(href) => …} />
export function makeAnofoxDashboard(React) {
  const { createElement, useEffect, useRef } = React;
  return function AnofoxDashboard({ src, getToken, vars, onState, onLink, title, style, className }) {
    const ref = useRef(null);
    const cbs = useRef({});
    cbs.current = { getToken, onState, onLink };
    useEffect(() => {
      const el = ref.current;
      el.getToken = (id) => cbs.current.getToken(id);
      const state = (e) => cbs.current.onState?.(e.detail.vars || {});
      const link = (e) => cbs.current.onLink?.(e.detail.href);
      el.addEventListener("anofox-state", state);
      el.addEventListener("anofox-link", link);
      if (src) el.setAttribute("src", src);
      return () => {
        el.removeEventListener("anofox-state", state);
        el.removeEventListener("anofox-link", link);
      };
    }, [src]);
    const varsKey = JSON.stringify(vars || null);
    useEffect(() => {
      if (vars) ref.current.setVars(vars);
    }, [varsKey]);
    return createElement("anofox-dashboard", { ref, title, style, class: className });
  };
}
