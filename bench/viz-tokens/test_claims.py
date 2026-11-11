"""#1116: python3 -m unittest bench/viz-tokens/test_claims.py (no API, no data)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from claims import claims, unsupported  # noqa: E402

ROWS = "month,rev2024,rev2025\n1,201589,267549\n5,233512,300683\n12,253059,312692\n"
DIGEST = ("line: 2024 — 12 marks, x 1 … 12; y first 201589, last 253059 | 2025 − 2024: "
          "largest +67171 (+28.765 %) at 5; by %: largest +65960 (+32.72 %) at 1")


class ClaimCheck(unittest.TestCase):
    def test_what_counts_as_a_claim(self):
        got = [c.text for c in claims(
            "In 2025 (from 2025-01-20 to 14:30) the top 3 regions earned €2.8M, 26.6 % more, "
            "about 26,000 units and 8.7 % of 1,500 products.")]
        self.assertEqual(got, ["2.8M", "26.6 %", "26,000", "8.7 %", "1,500"])

    def test_numbers_from_the_data_are_backed(self):
        self.assertEqual(unsupported("January rose by 32.7 %, the most; May gained 67,171.",
                                     [ROWS, DIGEST]), [])

    def test_a_rounded_or_derived_number_is_backed(self):
        # 312692 / 253059 − 1 = 23.6 %; 267549 − 201589 = 65960 ≈ 66,000
        self.assertEqual(unsupported("December grew 23.6 %; January added about 66,000.",
                                     [ROWS]), [])

    def test_a_share_of_a_fraction_is_backed(self):
        self.assertEqual(unsupported("Seals delivers 32.9 % on time.", ["name,on_time\nSeals,0.329\n"]), [])

    def test_a_number_nothing_backs_is_reported(self):
        self.assertEqual(unsupported("January had the smallest uplift, 18.8 %, and 410,000 in total.",
                                     [ROWS, DIGEST]), ["18.8 %", "410,000"])

    def test_a_column_total_is_backed(self):
        # 201589 + 233512 + 253059 = 688160
        self.assertEqual(unsupported("2024 brought 688,160 in these months.", [ROWS]), [])

    def test_the_request_counts_as_evidence(self):
        self.assertEqual(unsupported("Districts beyond 200 km still reach 91.7 %.",
                                     ["district,km,rate\nHeidelberg,225.0,0.917\n",
                                      "Which districts more than 200 km from …"]), [])


if __name__ == "__main__":
    unittest.main()
