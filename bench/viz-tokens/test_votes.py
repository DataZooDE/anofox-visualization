import unittest

from votes import majority, self_agreement


def v(nc, ca, r=4, why="x"):
    return {"numbers_correct": nc, "chart_answers": ca, "readable": r, "reason": why}


class VotesTest(unittest.TestCase):
    def test_unanimous_is_sure(self):
        m = majority([v(True, True), v(True, True), v(True, True)])
        self.assertTrue(m["numbers_correct"] and m["chart_answers"])
        self.assertFalse(m["unsure"])

    def test_split_is_majority_and_unsure(self):
        m = majority([v(True, True, why="ok"), v(False, True, why="bad number"), v(True, True, why="ok")])
        self.assertTrue(m["numbers_correct"] and m["chart_answers"])
        self.assertTrue(m["unsure"])
        self.assertEqual(m["reason"], "ok")

    def test_failed_call_makes_it_unsure(self):
        m = majority([v(True, True), v(None, None, None, "judge reply not JSON")])
        self.assertTrue(m["unsure"])

    def test_readable_median_and_self_agreement(self):
        self.assertEqual(majority([v(True, True, 2), v(True, True, 5), v(True, True, 4)])["readable"], 4)
        js = [{"votes": [1, 2, 3], "unsure": False}, {"votes": [1, 2, 3], "unsure": True}, {"reason": "no answer"}]
        self.assertEqual(self_agreement(js), 0.5)


if __name__ == "__main__":
    unittest.main()
