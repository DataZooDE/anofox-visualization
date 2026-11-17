"""#1263: python3 -m unittest bench/viz-tokens/test_shape.py — the Python
digest's shape parts say exactly what ggplot-rs's summary says (the expected
strings are ggplot-rs 0.60's output for the same series)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from shape import leaders_suffix, shape_suffix  # noqa: E402


def num(v):
    return str(int(v)) if float(v).is_integer() else f"{v:g}"


def line(ys):
    return shape_suffix([(str(i), float(y)) for i, y in enumerate(ys, 1)], num)


class Shape(unittest.TestCase):
    def test_as_ggplot_rs_writes_it(self):
        self.assertEqual(line([1, 2, 5, 3, 4, 6]), "; against the trend: largest fall 5 at 3 → 3 at 4; 2 peaks: 6 at 6, 5 at 3")
        self.assertEqual(line([9, 8, 4, 7, 3, 1]), "; against the trend: largest rise 4 at 3 → 7 at 4; 2 peaks: 9 at 1, 7 at 4")
        self.assertEqual(line([0, 9, 0, 8, 0, 7, 0, 10, 0]),
                         "; against the trend: largest fall 10 at 8 → 0 at 9; 4 peaks: 10 at 8, 9 at 2, 8 at 4, …")

    def test_steady_short_or_flat_says_nothing(self):
        self.assertEqual(line([0, 50, 49, 100]), "")
        self.assertEqual(line([1, 2, 3]), "")
        self.assertEqual(line([4, 4, 4, 4]), "")

    def test_who_leads_per_x(self):
        wi = [("a", 5.0), ("b", 6.0), ("c", 7.0), ("d", 1.0)]
        ca = [("a", 4.0), ("b", 5.0), ("c", 6.0), ("d", 2.0)]
        self.assertEqual(leaders_suffix([("WI", wi), ("CA", ca)]), "; highest at each of 4 shared x: WI 3, CA 1")
        self.assertEqual(leaders_suffix([("WI", wi)]), "")
        self.assertEqual(leaders_suffix([("WI", wi[:2]), ("CA", ca[:2])]), "")


if __name__ == "__main__":
    unittest.main()
