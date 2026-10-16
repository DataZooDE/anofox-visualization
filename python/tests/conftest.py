import os
import warnings

import numpy as np
import pandas as pd
import pytest

warnings.filterwarnings("ignore", category=FutureWarning)


@pytest.fixture
def data():
    rng = np.random.default_rng(7)
    n = 120
    d = pd.DataFrame(
        {
            "x1": rng.normal(size=n),
            "x2": rng.normal(size=n),
            "g": rng.choice(["a", "b", "c"], n),
        }
    )
    d["y"] = 1 + 2 * d.x1 - d.x2 + rng.normal(size=n)
    d["label"] = (d.y + rng.normal(size=n) > 1).astype(int)
    return d


@pytest.fixture(scope="session")
def con():
    """A connection with the extension; integration tests skip without it."""
    path = os.environ.get("ANOFOX_VIZ_EXTENSION")
    if not path or not os.path.exists(path):
        pytest.skip("set ANOFOX_VIZ_EXTENSION to an anofox_visualization.duckdb_extension")
    import anofox_viz as av

    return av.connect(path)
