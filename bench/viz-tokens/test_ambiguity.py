import unittest

from ambiguity import diverge, label


class AmbiguityTest(unittest.TestCase):
    def test_one_reading_is_clear(self):
        self.assertEqual(label([[("a", 1)]]), "clear")

    def test_close_numbers_same_order_are_harmless(self):
        a = [("Castings", 14.99), ("Seals", 6.15)]
        b = [("Castings", 15.6), ("Seals", 6.4)]
        self.assertEqual(label([a, b]), "harmless")

    def test_another_ranking_is_ambiguous(self):
        a = [("store", 9.1e6), ("app", 8.9e6)]
        b = [("app", 9.0e6), ("store", 8.8e6)]
        self.assertEqual(label([a, b]), "ambiguous")

    def test_numbers_far_apart_are_ambiguous(self):
        # seasonality: monthly totals vs per item-store row, same shape
        self.assertEqual(label([[(3, 1330.2)], [(3, 59.9)]]), "ambiguous")
        # late lines to the first vs the last receipt
        self.assertTrue(diverge([("Seals", 165)], [("Seals", 191)]))

    def test_row_count_and_nulls(self):
        self.assertTrue(diverge([("a", 1)], [("a", 1), ("b", 2)]))
        self.assertFalse(diverge([("a", None)], [("a", None)]))
        self.assertTrue(diverge([("a", None)], [("a", 0.5)]))


if __name__ == "__main__":
    unittest.main()
