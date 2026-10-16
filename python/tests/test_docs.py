"""Run the Python snippets of docs/python-r.md in order, in one namespace.

A block right after an ``<!-- not run`` comment is skipped; a block whose
library is not installed is skipped too. Every ``svg`` / ``svgs`` a block
leaves behind must be an SVG.
"""

import os
import re
from pathlib import Path

import pytest

DOC = Path(__file__).resolve().parents[2] / "docs" / "python-r.md"
BLOCK = re.compile(r"(<!-- not run[^>]*-->\s*)?```python\n(.*?)```", re.S)
BLOCKS = [(m.start(), m.group(2)) for m in BLOCK.finditer(DOC.read_text()) if not m.group(1)]


@pytest.fixture(scope="module")
def namespace(con, tmp_path_factory):
    # The snippets LOAD './anofox_visualization.duckdb_extension'.
    workdir = tmp_path_factory.mktemp("docs")
    (workdir / "anofox_visualization.duckdb_extension").symlink_to(
        os.path.abspath(os.environ["ANOFOX_VIZ_EXTENSION"])
    )
    cwd = os.getcwd()
    os.chdir(workdir)
    yield {"__name__": "docs"}
    os.chdir(cwd)


def _line(offset):
    return DOC.read_text()[:offset].count("\n") + 1


def test_docs_have_snippets():
    assert len(BLOCKS) >= 10


@pytest.mark.parametrize("offset,code", BLOCKS, ids=[f"line{_line(o)}" for o, _ in BLOCKS])
def test_snippet(namespace, offset, code):
    namespace.pop("svg", None)
    namespace.pop("svgs", None)
    try:
        exec(compile(code, f"python-r.md:{_line(offset)}", "exec"), namespace)
    except ModuleNotFoundError as e:
        pytest.skip(f"needs {e.name}")
    if "svg" in namespace:
        assert str(namespace["svg"]).startswith("<svg")
    for v in namespace.get("svgs", {}).values():
        assert v.startswith("<svg")
