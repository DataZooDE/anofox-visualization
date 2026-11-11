"""#1122: python3 -m unittest bench/viz-tokens/test_stats.py (no API, no data)."""
import math
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from stats import diff_ci, fmt_diff, fmt_rate, rate_ci, verdict, wilson  # noqa: E402


class Wilson(unittest.TestCase):
    def test_known_values(self):
        # 8 of 10: the textbook Wilson 95 % interval is 0.490–0.943
        lo, hi = wilson(8, 10)
        self.assertAlmostEqual(lo, 0.4902, places=3)
        self.assertAlmostEqual(hi, 0.9433, places=3)

    def test_edges(self):
        lo, hi = wilson(10, 10)
        self.assertEqual(hi, 1.0)
        self.assertLess(lo, 1.0)  # all passed is still not certainty
        self.assertEqual(wilson(0, 10)[0], 0.0)
        self.assertTrue(all(math.isnan(x) for x in wilson(0, 0)))


class RateCI(unittest.TestCase):
    def test_counts_and_point(self):
        s = rate_ci({"a": [True, True, False], "b": [True], "c": []})
        self.assertEqual((s["pass"], s["runs"], s["tasks"]), (3, 4, 2))
        self.assertAlmostEqual(s["rate"], 0.75)
        self.assertLessEqual(s["boot"][0], s["rate"])
        self.assertGreaterEqual(s["boot"][1], s["rate"])

    def test_deterministic(self):
        t = {f"t{i}": [i % 3 != 0] * 3 for i in range(30)}
        self.assertEqual(rate_ci(t)["boot"], rate_ci(t)["boot"])

    def test_clustered_is_wider_than_wilson_when_repeats_agree(self):
        # 3 identical repeats per task: 90 runs but only 30 independent answers
        t = {f"t{i}": [i % 3 != 0] * 3 for i in range(30)}
        s = rate_ci(t)
        boot = s["boot"][1] - s["boot"][0]
        wil = s["wilson"][1] - s["wilson"][0]
        self.assertGreater(boot, wil * 1.3)
        # and about the width of one run per task
        one = rate_ci({k: v[:1] for k, v in t.items()})
        self.assertAlmostEqual(boot, one["boot"][1] - one["boot"][0], delta=0.05)

    def test_all_pass(self):
        s = rate_ci({f"t{i}": [True] * 3 for i in range(10)})
        self.assertEqual(s["boot"], [1.0, 1.0])
        self.assertLess(s["wilson"][0], 1.0)
        self.assertIn("30/30 100 %", fmt_rate(s))

    def test_empty(self):
        s = rate_ci({})
        self.assertEqual(s["runs"], 0)
        self.assertEqual(fmt_rate(s), "–")


class DiffCI(unittest.TestCase):
    def test_paired_on_shared_tasks_only(self):
        a = {"x": [True] * 3, "y": [False] * 3, "only-a": [True]}
        b = {"x": [False], "y": [False], "only-b": [True]}
        d = diff_ci(a, b)
        self.assertTrue(d["paired"])
        self.assertEqual(d["tasks"], 2)
        self.assertAlmostEqual(d["diff"], 0.5)  # (1 − 0 + 0 − 0) / 2

    def test_tasks_weigh_the_same_whatever_the_repeats(self):
        a = {"x": [True, True, False], "y": [True]}
        b = {"x": [True], "y": [True, False, False]}
        self.assertAlmostEqual(diff_ci(a, b)["diff"], ((2 / 3 - 1) + (1 - 1 / 3)) / 2)

    def test_identical_runs_are_within_noise(self):
        t = {f"t{i}": [i % 4 != 0, i % 5 != 0, True] for i in range(40)}
        d = diff_ci(t, t)
        self.assertEqual(d["diff"], 0)
        self.assertEqual(d["ci"], [0, 0])
        self.assertEqual(verdict(d), "within noise")

    def test_clear_gain_is_better_and_loss_is_worse(self):
        good = {f"t{i}": [True] * 3 for i in range(40)}
        bad = {f"t{i}": [i % 2 == 0] * 3 for i in range(40)}
        up = diff_ci(good, bad)
        self.assertAlmostEqual(up["diff"], 0.5)
        self.assertGreater(up["ci"][0], 0)
        self.assertEqual(verdict(up), "better")
        self.assertEqual(verdict(diff_ci(bad, good)), "worse")

    def test_small_noisy_change_is_within_noise(self):
        # 2 of 20 tasks flip one way, 1 the other: +5 points, nowhere near significant
        base = {f"t{i}": [i % 3 != 0] for i in range(20)}
        now = dict(base, t0=[True], t3=[True], t1=[False])
        d = diff_ci(now, base)
        self.assertAlmostEqual(d["diff"], 0.05)
        self.assertEqual(verdict(d), "within noise")

    def test_unpaired_fallback(self):
        d = diff_ci({"a": [True], "b": [True]}, {"c": [False], "d": [False]})
        self.assertFalse(d["paired"])
        self.assertAlmostEqual(d["diff"], 1.0)
        self.assertEqual(verdict(d), "better")

    def test_empty_side(self):
        d = diff_ci({"a": [True]}, {})
        self.assertEqual(d["tasks"], 0)
        self.assertEqual(fmt_diff(d), "–")

    def test_format(self):
        self.assertEqual(fmt_diff({"diff": 0.06, "ci": [-0.021, 0.14], "tasks": 5}), "+6.0 pts (-2.1 … +14.0)")


if __name__ == "__main__":
    unittest.main()
