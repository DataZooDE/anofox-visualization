"""#1116: python3 -m unittest bench/viz-tokens/test_claims.py (no API, no data)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from claims import claims, latest_digest, unsupported, word_problems  # noqa: E402

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



# #1263: digests as the anofox CLI writes them (ggplot-rs summary ≥ 0.60)
DIP = ("statement 1: col: 12 marks, x 2025-01-01 … 2025-12-01; y first 267549, last 312692, min 267549 at "
       "2025-01-01, max 312692 at 2025-12-01; against the trend: largest fall 300686 at 2025-05-01 → 294410 at 2025-09-01")
BLIP = ("statement 1: line: US — 36 marks, x 2023-01-01 … 2025-12-01; y first 100, last 232.2, min 100 at 2023-01-01, "
        "max 232.2 at 2025-12-01; against the trend: largest fall 136.79 at 2023-04-01 → 119.34 at 2023-07-01")
PEAKS = ("statement 1: line: Factory — 52 marks, x 1 … 52; y first 0, last 0, min 0 at 1, max 188.23 at 16; "
         "against the trend: largest fall 188.23 at 16 → 0 at 21; 3 peaks: 188.23 at 16, 74.433 at 29, 50.1 at 7; "
         "Retailer — 52 marks, x 1 … 52; y first 0, last 0, min 0 at 1, max 119.3 at 16; against the trend: "
         "largest fall 119.3 at 16 → 0 at 19")
FACETS = ("statement 1: line [CA_1]: 51 marks, x 2011-02-01 … 2015-04-01; y first 252, last 532, min 134 at 2012-01-01, "
          "max 739 at 2013-03-01 | line [WI_1]: 51 marks, x 2011-02-01 … 2015-04-01; y first 292, last 613, min 154 at "
          "2011-08-01, max 911 at 2012-08-01 | line across panels: highest at each of 51 shared x: WI_1 25, CA_1 22, TX_1 4")


class WordCheck(unittest.TestCase):
    def test_steady_growth_over_a_long_dip_is_reported(self):
        got = word_problems("Revenue rises steadily from $267,549 to $312,692.", DIP)
        self.assertEqual([p for p, _ in got], ["rises steadily"])
        self.assertIn("fall 300686 at 2025-05-01 → 294410 at 2025-09-01", got[0][1])

    def test_a_short_blip_in_a_long_rise_is_not(self):
        self.assertEqual(word_problems("The US index shows steady upward growth to 232.", BLIP), [])

    def test_back_to_zero_for_good_before_a_second_peak_is_reported(self):
        got = word_problems("Backlog peaks at week 16, then falls back near zero for the rest of the year.", PEAKS)
        self.assertEqual(len(got), 1)
        self.assertIn("Factory has 3 peaks", got[0][1])

    def test_consistently_highest_needs_every_shared_x(self):
        got = word_problems("All stores grow, with WI_1 consistently having the highest volume.", FACETS)
        self.assertEqual(got[0][1], "WI_1 is highest at 25 of 51 x")
        # a 'consistently lowest' or another store is not this rule's claim
        self.assertEqual(word_problems("TX_1 is consistently the lowest.", FACETS), [])

    def test_no_known_phrase_or_no_digest_passes(self):
        self.assertEqual(word_problems("Revenue grew 16.9 % over the year.", DIP), [])
        self.assertEqual(word_problems("Revenue rises steadily.", ""), [])
        # an old digest without the shape parts says nothing against it
        old = DIP.split("; against the trend")[0]
        self.assertEqual(word_problems("Revenue rises steadily.", old), [])

    def test_the_latest_chart_is_the_one_checked(self):
        ev = ["question", "ok: drawn\nwhat the chart shows (quote these numbers in the title and answer):\n" + BLIP,
              "month,rev\n1,2", "ok: drawn\nwhat the chart shows (quote these numbers in the title and answer):\n" + DIP]
        self.assertIn("2025-05-01", latest_digest(ev))

if __name__ == "__main__":
    unittest.main()
