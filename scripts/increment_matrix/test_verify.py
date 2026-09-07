"""Acceptance reports must not turn incomplete evidence into green runs."""
import copy
import unittest

from verify import verify_delta, verify_rvm, business_residuals


def element(ref="1/1", x=0):
    return {"geometry_id": {"kind": "element", "refno": ref}, "noun": "BOX",
            "aabb_min": [x, 0, 0], "aabb_max": [x + 1, 1, 1],
            "world": [[1, 0, 0], [0, 1, 0], [0, 0, 1], [x, 0, 0]],
            "volume_mm3": 1, "triangles": 12, "genus": 0, "components": []}


class DeltaAcceptance(unittest.TestCase):
    def test_missing_changed_upsert_is_a_failure(self):
        result = verify_delta([element()], [element(x=5)], [], [])
        self.assertFalse(result["passed"])
        self.assertEqual(len(result["changed_not_upserted"]), 1)

    def test_deleted_member_must_be_removed(self):
        self.assertFalse(verify_delta([element()], [], [], [])["passed"])
        self.assertTrue(verify_delta([element()], [], [], [element()["geometry_id"]])["passed"])

    def test_full_matching_move_passes_but_wrong_transform_fails(self):
        target = element(x=5)
        self.assertTrue(verify_delta([element()], [target], [target], [])["passed"])
        wrong = copy.deepcopy(target)
        wrong["world"][3][0] = 6
        self.assertFalse(verify_delta([element()], [target], [wrong], [])["passed"])

    def test_duplicate_geometry_ids_are_rejected(self):
        with self.assertRaises(ValueError):
            verify_delta([], [element(), element()], [element()], [])


class RvmAcceptance(unittest.TestCase):
    def test_empty_or_missing_evidence_cannot_pass(self):
        for report in ({}, {"pairs": [], "only_generated": [], "only_baseline": []}):
            with self.subTest(report=report):
                self.assertFalse(verify_rvm(report)["passed"])

    def test_off_or_unpaired_geometry_fails(self):
        report = {"pairs": [{"verdict": "exact", "max_axis_delta_mm": 0.01}],
                  "only_generated": [], "only_baseline": [],
                  "unkeyable_generated": [], "unkeyable_baseline": []}
        self.assertTrue(verify_rvm(report)["passed"])
        report["pairs"][0]["verdict"] = "off"
        self.assertFalse(verify_rvm(report)["passed"])
        report["pairs"][0]["verdict"] = "exact"
        report["only_baseline"] = ["missing"]
        self.assertFalse(verify_rvm(report)["passed"])


class RestoreAcceptance(unittest.TestCase):
    def test_only_known_save_metadata_is_ignored(self):
        rows = [{"noun": "DBMDAT", "op": "modified", "modified_explicit": ["DBMVAL"]},
                {"noun": "BRAN", "op": "modified", "modified_explicit": ["CACHID"]}]
        self.assertEqual(business_residuals({"ops": rows}), [])
        rows.append({"noun": "DAMP", "op": "modified", "modified_explicit": ["UNIPAR"]})
        self.assertEqual(business_residuals({"ops": rows}), rows[-1:])


if __name__ == "__main__":
    unittest.main()
