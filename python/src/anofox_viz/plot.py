"""Render contract tables to SVG through the DuckDB extension."""

from __future__ import annotations

import math
import os
import re
import uuid
from pathlib import Path
from typing import Any, Mapping

__all__ = ["SVG", "connect", "plot", "CHARTS"]

#: ``chart`` → macro ``anofox_plot_<chart>``; ``None`` lets ``anofox_plot``
#: pick from the columns.
CHARTS = (
    "terms",
    "prediction",
    "curve",
    "summary",
    "obs",
    "diagnostics",
    "tests",
    "compare",
    "partial",
)

# Which table of a shaped result (`from_statsmodels(...)`) a chart plots.
_TABLE_FOR_CHART = {"diagnostics": "obs", "compare": None, "partial": None}

#: Repository of the project's extension builds (DuckDB v1.5.6, unsigned).
RELEASE_REPO = "https://datazoode.github.io/anofox-visualization/extensions"

_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")

_default_con: Any = None


class SVG(str):
    """An SVG string that Jupyter displays as a picture."""

    def _repr_svg_(self) -> str:
        return str(self)

    def save(self, path: str | os.PathLike) -> Path:
        p = Path(path)
        p.write_text(self, encoding="utf-8")
        return p


def _loaded(con: Any) -> bool:
    try:
        con.execute("SELECT anofox_contract_version()").fetchone()
        return True
    except Exception:
        return False


def connect(extension: str | os.PathLike | None = None, con: Any = None) -> Any:
    """A DuckDB connection with anofox_visualization loaded.

    ``extension`` is the path of an ``anofox_visualization.duckdb_extension``
    file; without one, ``$ANOFOX_VIZ_EXTENSION``, an already installed
    extension, the community repository and the project's release
    repository are tried in that order. ``con`` loads into an existing
    connection (it needs ``allow_unsigned_extensions`` for a local file).
    The connection becomes the one :func:`plot` uses without ``con=``.
    """
    import duckdb

    global _default_con
    if con is None:
        con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
    _load(con, extension)
    _default_con = con
    return con


def _load(con: Any, extension: str | os.PathLike | None) -> None:
    if _loaded(con):
        return
    path = extension or os.environ.get("ANOFOX_VIZ_EXTENSION")
    if path:
        con.execute(f"LOAD {_literal(str(path))}")
        return
    errors = []
    for install in (
        None,
        "INSTALL anofox_visualization FROM community",
        f"INSTALL anofox_visualization FROM {_literal(RELEASE_REPO)}",
    ):
        try:
            if install:
                con.execute(install)
            con.execute("LOAD anofox_visualization")
            return
        except Exception as e:  # try the next source
            errors.append(f"{install or 'LOAD'}: {e}")
    raise RuntimeError(
        "anofox_visualization could not be loaded; pass connect(extension='…path…') "
        "or set ANOFOX_VIZ_EXTENSION.\n" + "\n".join(errors)
    )


def _connection(con: Any) -> Any:
    if con is not None:
        _load(con, None)
        return con
    return _default_con if _default_con is not None else connect()


def _literal(v: Any) -> str:
    """A Python value as a SQL literal (a macro argument)."""
    if v is None:
        return "NULL"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(int(v))
    if isinstance(v, float):
        if not math.isfinite(v):
            raise ValueError(f"option value {v!r} is not finite")
        return repr(float(v))
    if isinstance(v, str):
        return "'" + v.replace("'", "''") + "'"
    if isinstance(v, (list, tuple)):
        return "[" + ", ".join(_literal(x) for x in v) + "]"
    try:  # numpy scalars
        return _literal(v.item())
    except AttributeError:
        pass
    raise TypeError(f"cannot pass {type(v).__name__} as a plot option")


def _is_table(v: Any) -> bool:
    """pandas / polars DataFrame or pyarrow Table (anything duckdb scans)."""
    return hasattr(v, "columns") and not isinstance(v, Mapping)


def plot(
    data: Any,
    chart: str | None = None,
    *,
    by: str | None = None,
    con: Any = None,
    **options: Any,
) -> SVG | dict[Any, SVG]:
    """Plot a contract table with the matching ``anofox_plot_*`` macro.

    ``data`` is a pandas or polars DataFrame or an Arrow table, or the dict
    a ``from_*`` function returns (then ``chart`` picks the table: ``terms``,
    ``obs`` / ``diagnostics``, ``summary``, ``prediction``, ``curve``,
    ``tests``). ``chart`` names the macro (``"terms"`` →
    ``anofox_plot_terms``); ``None`` lets ``anofox_plot`` choose from the
    columns. Keyword options are passed to the macro (``title="…"``,
    ``kind="qq"``, ``alpha=0.01``, ``metrics=["aic", "bic"]``); a DataFrame
    option (``tests=`` for ``compare``) is passed as a list of its rows.

    Returns one :class:`SVG`, or with ``by="column"`` a dict of one SVG per
    value of that column (e.g. ``by="curve_type"``).
    """
    if chart is not None and chart not in CHARTS:
        raise ValueError(f"chart must be one of {', '.join(CHARTS)} or None, got {chart!r}")
    if isinstance(data, Mapping):
        key = _TABLE_FOR_CHART.get(chart, chart) if chart else None
        if key is None or key not in data:
            raise ValueError(
                f"pick a table: plot(result, chart) with chart one of {', '.join(data)}"
                if key is None
                else f"no {key!r} table here (got {', '.join(data)})"
            )
        data = data[key]
    if not _is_table(data):
        raise TypeError(f"expected a DataFrame or Arrow table, got {type(data).__name__}")
    if by is not None and not _IDENT.match(by):
        raise ValueError(f"by={by!r} must be a plain column name")

    con = _connection(con)
    tag = uuid.uuid4().hex[:12]
    registered = [f"anofox_input_{tag}"]
    con.register(registered[0], data)
    try:
        args = ["anofox_row_"]
        for name, value in options.items():
            if not _IDENT.match(name):
                raise ValueError(f"bad option name {name!r}")
            if _is_table(value):
                t = f"anofox_option_{tag}_{len(registered)}"
                con.register(t, value)
                registered.append(t)
                args.append(f"{name} := (SELECT list(o) FROM {t} o)")
            else:
                args.append(f"{name} := {_literal(value)}")
        macro = f"anofox_plot_{chart}" if chart else "anofox_plot"
        call = f"{macro}({', '.join(args)})"
        src = f"FROM {registered[0]} AS anofox_row_"
        if by is None:
            return SVG(con.execute(f"SELECT {call} {src}").fetchone()[0])
        rows = con.execute(f'SELECT "{by}", {call} {src} GROUP BY ALL ORDER BY 1').fetchall()
        return {k: SVG(v) for k, v in rows}
    finally:
        for t in registered:
            con.unregister(t)
