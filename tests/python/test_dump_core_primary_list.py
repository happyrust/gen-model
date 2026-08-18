import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "e3d" / "dump_core_primary_list.py"
SPEC = importlib.util.spec_from_file_location("dump_core_primary_list", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class CorePrimaryListDumpTests(unittest.TestCase):
    def test_noun_rows_normalize_filter_and_reject_duplicates(self):
        nouns = [
            {"noun_hash": 1, "noun_name": " damp "},
            {"noun_hash": 2, "noun_name": "TP"},
            {"noun_hash": 3, "noun_name": ""},
        ]
        self.assertEqual(
            MODULE.noun_rows(nouns, {"DAMP"}), [{"hash": 1, "noun": "DAMP"}]
        )
        with self.assertRaisesRegex(ValueError, "duplicate"):
            MODULE.noun_rows(nouns + [{"noun_hash": 4, "noun_name": "DAMP"}])

    def test_payload_uses_strict_value_equals_one_and_keeps_unknown_separate(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            core = root / "core.dll"
            nouns = root / "noun_flags.json"
            core.write_bytes(b"core-fixture")
            nouns.write_text(json.dumps([]), encoding="utf-8")
            payload = MODULE.build_payload(
                [
                    {"noun": "DAMP", "hash": 1, "ok": True, "value": 1},
                    {"noun": "TP", "hash": 2, "ok": True, "value": 0},
                    {"noun": "MDB", "hash": 3, "ok": True, "value": 2},
                    {"noun": "ROD", "hash": 4, "ok": False, "value": 0},
                ],
                core,
                nouns,
            )

        self.assertEqual(payload["count"], 4)
        self.assertEqual(payload["resolved_count"], 3)
        self.assertEqual(payload["unknown_count"], 1)
        self.assertEqual(payload["true_count"], 1)
        self.assertEqual(payload["false_count"], 2)
        self.assertEqual(payload["nouns"], {"DAMP": True, "TP": False, "MDB": False})
        self.assertEqual(payload["unknown"], [{"noun": "ROD", "hash": 4}])
        self.assertEqual(
            payload["non_binary_values"], [{"noun": "MDB", "hash": 3, "value": 2}]
        )


if __name__ == "__main__":
    unittest.main()
