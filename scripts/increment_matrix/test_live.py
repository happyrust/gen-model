import unittest

from live import compare_records, converged


class LiveAcceptanceTests(unittest.TestCase):
    def test_a_current_data_watermark_does_not_hide_model_lag_or_pending(self):
        row = {"applied_sesno": 12, "model_sesno": 11, "model_verdict": "lagging"}
        pending = {"units": [], "dead_letters": 0}
        self.assertFalse(converged(row, pending, 12))
        row.update(model_sesno=12, model_verdict='in_sync')
        self.assertTrue(converged(row, pending, 12))
        self.assertFalse(converged(row, {**pending, "units": [{"refno": "1/2"}]}, 12))
        self.assertFalse(converged(row, {**pending, "dead_letters": 1}, 12))
        self.assertFalse(converged({}, pending, 12))

    def test_same_count_with_wrong_identity_or_bounds_is_not_convergence(self):
        expected = [{"refno": "1/2", "geometry_id": {"kind": "element"},
                     "aabb_min": [0, 0, 0], "aabb_max": [1, 1, 1]}]
        record = {"refno": "1_2", "world_aabb": {"mins": [0, 0, 0], "maxs": [1, 1, 1]},
                  "insts": [{"is_tubi": False}]}
        self.assertTrue(compare_records(expected, [record])['passed'])
        self.assertFalse(compare_records(expected, [{**record, "refno": "1_3"}])['passed'])
        self.assertFalse(compare_records(expected, [{**record, "world_aabb": {
            "mins": [0, 0, 0], "maxs": [2, 1, 1]}}])['passed'])

    def test_deleted_geometry_cannot_pass_with_an_old_published_record(self):
        stale = {"refno": "1_2", "world_aabb": {"mins": [0, 0, 0], "maxs": [1, 1, 1]},
                 "insts": [{"is_tubi": False}]}
        self.assertTrue(compare_records([], [])['passed'])
        self.assertFalse(compare_records([], [stale])['passed'])


if __name__ == '__main__':
    unittest.main()
