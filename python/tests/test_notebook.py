"""Execute examples/python/plot-results.ipynb top to bottom.

Skipped without the extension (``ANOFOX_VIZ_EXTENSION``) or without the
notebook's libraries (nbclient, ipykernel, statsforecast, pingouin, …).
"""

import os
import shutil
from pathlib import Path

import pytest

NOTEBOOK = Path(__file__).resolve().parents[2] / "examples" / "python" / "plot-results.ipynb"


def test_notebook_runs(tmp_path):
    path = os.environ.get("ANOFOX_VIZ_EXTENSION")
    if not path or not os.path.exists(path):
        pytest.skip("set ANOFOX_VIZ_EXTENSION to an anofox_visualization.duckdb_extension")
    for mod in ("nbclient", "nbformat", "ipykernel", "statsmodels", "sklearn", "scipy",
                "pingouin", "statsforecast", "pyarrow"):
        pytest.importorskip(mod)
    import nbformat
    from nbclient import NotebookClient

    # Run a copy: the notebook writes results/ and shop_report.sql next to itself.
    work = tmp_path / "examples" / "python"
    work.mkdir(parents=True)
    shutil.copy(NOTEBOOK, work)
    nb = nbformat.read(work / NOTEBOOK.name, as_version=4)
    NotebookClient(nb, timeout=300, kernel_name="python3",
                   resources={"metadata": {"path": str(work)}}).execute()

    svgs = [o["data"]["image/svg+xml"] for c in nb.cells if c.cell_type == "code"
            for o in c.outputs if "image/svg+xml" in o.get("data", {})]
    assert len(svgs) >= 10 and all(s.lstrip().startswith("<svg") for s in svgs)
    assert len(list((work / "results").glob("*.parquet"))) == 7
    assert (work / "shop_report.sql").exists()
