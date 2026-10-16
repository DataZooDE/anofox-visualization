"""Shape Python model results into the anofox contract and plot them.

    import anofox_viz as av
    t = av.from_statsmodels(res)          # {"terms", "obs", "summary", "prediction"}
    av.plot(t, "terms")                   # SVG; shown inline in Jupyter
"""

from .plot import CHARTS, SVG, connect, plot
from .shape import (
    from_estimator,
    from_lifelines,
    from_pingouin,
    from_prophet,
    from_scipy,
    from_sklearn,
    from_statsforecast,
    from_statsmodels,
    from_tukeyhsd,
)

__version__ = "0.1.0"

__all__ = [
    "CHARTS",
    "SVG",
    "connect",
    "plot",
    "from_estimator",
    "from_lifelines",
    "from_pingouin",
    "from_prophet",
    "from_scipy",
    "from_sklearn",
    "from_statsforecast",
    "from_statsmodels",
    "from_tukeyhsd",
]
