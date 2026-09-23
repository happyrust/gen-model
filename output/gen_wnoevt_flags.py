# -*- coding: utf-8 -*-
"""Decode the per-attribute WNOEVT / WNOCLM dictionary flags straight out of
`attlib.dat`, offline — no live E3D session.

Why this exists
---------------
`DB_Attribute::ReadData` (core.dll 0x58CE1F0) fills every meta-field of an
attribute (DCHC, PLCF, WNOEVT, WNOCLM, ...) through one reader,
`sub_55BB6C3` / `sub_55BB60C` (PDMS ATNLOG / "AXALOG"), keyed by
`(attribute-hash, field-hash)`. `WNOEVT` (`299311034`, lands at `this+184`;
`DB_Attribute::wnoevt` 0x58D5290 returns it) is the gate the
`DB_ElementChangesPlugger::Post*` broadcasters consult first: a put on a
WNOEVT attribute fires NO change event at all — before DCHC even matters.

Reversing `sub_55BB6C3` + the ATTOPE loader (`sub_55F4290`) via ida-bridge
(core.dll = idalib-33060, E3D 3.1) showed the reader's tables are exactly the
`attlib.dat` sections `e3d-attlib` already parses:

  * ATGTDF (directory page[0]) -> the ordered meta-field list `dword_6BE26E0`
    (field hash -> 1-based ordinal) and count N (`dword_6C08EB4` = 63).
  * ATGTIX (directory page[2]) -> the attribute index: sorted hash
    `dword_6BCA6E0`, record page `dword_6BD26E0`, record base `dword_6BDA6E0`,
    encoded on disk as `(hash, packed = page*512 + word_offset)`.
  * The record itself is one "DB,BL 512" block (`FHDBRN`): a header of N
    per-field offsets followed by the values. For field ordinal `o`:

        row       = (page-1)*512 + base - 2          # 0-based file words
        off       = word[row + o]                    # header slot
        value     = word[row + off]                  # -1 => schema default (0),
                                                     #  0 => inherited (0)

This is validated below: decoding DCHC (ordinal 49) reproduces the committed
`dchc_change_classes.json` for all 4270 names, and PLCF (ordinal 50) reproduces
`plcf_flags.json` exactly (58/58). WNOEVT/WNOCLM ride the identical pipeline.

The same decode is also implemented in Rust as the canonical, reusable form:
`vendor/e3d-attlib/src/dict.rs` (`e3d_attlib::dict::AttrDict`), with a portable
calibration test (`vendor/e3d-attlib/tests/dict_calibration.rs`) and a runnable
dump (`cargo run --example wnoevt -- <attlib.dat>`). This Python generator stays
the fixture producer wired into the explorer; both agree on WNOEVT=7 / WNOCLM=6.

Result (E3D 3.1 attlib.dat): WNOEVT is set on exactly 7 attributes
(DPOSCA LIGTCA EVITCA TEMPLI LIGTCP EVITCP EXTEMP) and WNOCLM on 6 — all
laser / lighting / temperature attributes; NONE are in the model-generation
schema, i.e. no attribute the increment logic ever sees is "silent".

Inputs : attlib.dat (E3D install; pass a path as argv[1], else auto-locate)
         src/data_interface/dchc_change_classes.json  (parity self-check)
         src/data_interface/plcf_flags.json            (parity self-check)
Output : src/data_interface/wnoevt_flags.json
"""
import json
import os
import struct
import sys
from collections import Counter

sys.stdout.reconfigure(encoding="utf-8")

ATTLIB_CANDIDATES = [
    r"E:\reverse\e3d\shadow_e3d31_gen_model_test\attlib.dat",
    r"C:\Program Files (x86)\AVEVA\Everything3D3.1\attlib.dat",
    r"D:\AVEVA\Everything3D3.1\attlib.dat",
]
WORDS_PER_PAGE = 512
DELIM = 0xFFFFFFFF
DCHC, PLCF, WNOEVT, WNOCLM = 596407, 652066, 299311034, 193514909


def locate_attlib() -> str:
    if len(sys.argv) > 1:
        return sys.argv[1]
    for path in ATTLIB_CANDIDATES:
        if os.path.isfile(path):
            return path
    sys.exit("attlib.dat not found; pass a path as argv[1] (tried "
             + ", ".join(ATTLIB_CANDIDATES) + ")")


def dehash(h: int):
    """Inverse of the PDMS base-27 name hash (ENHAS2): A-Z=1..26, space=0."""
    if not (531442 <= h <= 387951929):
        return None
    v = h - 531441
    out = []
    for _ in range(6):
        d = v % 27
        out.append(" " if d == 0 else chr(64 + d))
        v //= 27
        if v == 0:
            break
    return "".join(out).rstrip()


class Attlib:
    def __init__(self, path: str):
        blob = open(path, "rb").read()
        self.n = len(blob) // 4
        self.w = struct.unpack(">%dI" % self.n, blob[: self.n * 4])
        directory = [self.w[WORDS_PER_PAGE + k] for k in range(8)]
        self.field_ordinal = self._parse_atgtdf(directory[0] - 1)
        self.index = self._parse_atgtix(directory[2] - 1)

    def _parse_atgtdf(self, page0: int) -> dict:
        fields, page, done, guard = [], page0, False, 0
        while not done and guard < 200:
            guard += 1
            base, i = page * WORDS_PER_PAGE, 0
            while i + 2 < WORDS_PER_PAGE:
                h = self.w[base + i]
                if h == DELIM:
                    done = True
                    break
                if h == 0:
                    break
                typ, kind = self.w[base + i + 1], self.w[base + i + 2]
                if not (531442 <= h <= 387951929 and 1 <= typ <= 12 and kind in (1, 2)):
                    break
                i += 3
                if kind == 2 and i < WORDS_PER_PAGE:
                    i += (1 + self.w[base + i]) if typ == 4 else 1
                fields.append(h)
            page += 1
        ordinal, seen = {}, set()
        for h in fields:
            if h not in seen:
                seen.add(h)
                ordinal[h] = len(ordinal) + 1
        return ordinal

    def _parse_atgtix(self, page0: int) -> dict:
        index, page, done, guard = {}, page0, False, 0
        while not done and guard < 4096:
            guard += 1
            base, i = page * WORDS_PER_PAGE, 0
            while i + 1 < WORDS_PER_PAGE:
                h = self.w[base + i]
                if h == DELIM:
                    done = True
                    break
                if h == 0:
                    break
                if not (531442 <= h <= 0xFFFFFFFF):
                    break
                packed = self.w[base + i + 1]
                index[h] = (packed // WORDS_PER_PAGE, packed % WORDS_PER_PAGE)
                i += 2
            page += 1
        return index

    def field(self, attr_hash: int, field_hash: int):
        """Raw dictionary value of `field_hash` on `attr_hash`, or None when the
        attribute/field is absent. -1 (schema default) and 0 (inherited) both
        surface as 0, matching how DCHC/PLCF resolve (proven by parity)."""
        ordinal = self.field_ordinal.get(field_hash)
        loc = self.index.get(attr_hash)
        if ordinal is None or loc is None:
            return None
        page, base = loc
        row = (page - 1) * WORDS_PER_PAGE + base - 2
        hi = row + ordinal
        if not (0 <= hi < self.n):
            return None
        off = self.w[hi]
        if off == DELIM or off == 0:
            return 0
        vi = row + off
        return self.w[vi] if 0 <= vi < self.n else None


def main() -> None:
    path = locate_attlib()
    attlib = Attlib(path)
    print(f"attlib: {path}  fields={len(attlib.field_ordinal)}  attrs={len(attlib.index)}")
    for name, h in (("DCHC", DCHC), ("PLCF", PLCF), ("WNOEVT", WNOEVT), ("WNOCLM", WNOCLM)):
        print(f"  ordinal[{name}] = {attlib.field_ordinal.get(h)}")

    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    di = os.path.join(root, "src", "data_interface")

    # name -> stored attribute hash, straight from the dictionary export (the
    # ATGTIX key is not always the base-27 hash of the short name, e.g. long
    # names like FTHICKNESS), same source gen_dchc_fixture.py resolves against.
    name2hash = {}
    for cand in (os.path.join(root, "output", "noun_attr_fields.json"),
                 os.path.join(root, "..", "vendor", "e3d-io", "schema", "e3d31", "catalog", "noun_attr_fields.json")):
        if os.path.isfile(cand):
            for key, rec in json.load(open(cand, encoding="utf-8")).items():
                nm = (rec.get("name") or "").strip().upper()
                if nm:
                    name2hash[nm] = int(key)
            break
    if not name2hash:
        sys.exit("noun_attr_fields.json not found; needed to map names -> stored hashes for the parity self-check.")

    # --- self-validation against committed snapshots ---
    dchc_gt = json.load(open(os.path.join(di, "dchc_change_classes.json"), encoding="utf-8"))
    dchc_ok = dchc_bad = 0
    for name, gt in dchc_gt.items():
        h = name2hash.get(name)
        if h is not None and attlib.field(h, DCHC) == gt:
            dchc_ok += 1
        else:
            dchc_bad += 1
    print(f"DCHC parity: {dchc_ok}/{len(dchc_gt)} (mismatch {dchc_bad})")
    if dchc_bad:
        sys.exit(f"DCHC parity failed ({dchc_bad}); attlib.dat version differs from the "
                 "committed snapshot — refusing to emit WNOEVT from a mismatched dictionary.")

    plcf_gt = set(json.load(open(os.path.join(di, "plcf_flags.json"), encoding="utf-8"))["names"])
    plcf_dec = {name for name in plcf_gt if name in name2hash and attlib.field(name2hash[name], PLCF) == 1}
    print(f"PLCF parity: {len(plcf_dec)}/{len(plcf_gt)}")
    if plcf_dec != plcf_gt:
        sys.exit("PLCF parity failed; refusing to emit from a mismatched dictionary.")

    # --- extract WNOEVT / WNOCLM over every attribute in the index ---
    def collect(field_hash: int):
        names = []
        for attr_hash in attlib.index:
            if attlib.field(attr_hash, field_hash) == 1:
                nm = dehash(attr_hash)
                if nm:
                    names.append(nm)
        return sorted(set(names))

    wnoevt = collect(WNOEVT)
    wnoclm = collect(WNOCLM)
    print(f"WNOEVT==1: {len(wnoevt)} -> {wnoevt}")
    print(f"WNOCLM==1: {len(wnoclm)} -> {wnoclm}")

    payload = {
        "field": "WNOEVT",
        "field_hash": WNOEVT,
        "reader": "core.dll DB_Attribute::ReadData 0x58CE1F0 -> sub_55BB6C3 (ATNLOG), this+184; DB_Attribute::wnoevt 0x58D5290 returns it. WNOEVT=1 => a put fires no change event (the gate before DCHC).",
        "source": "attlib.dat offline decode (ATGTDF field list + ATGTIX record index); DCHC parity 4270/4270 and PLCF parity 58/58 vs the committed snapshots. Generator: output/gen_wnoevt_flags.py",
        "note": "None of these are in the model-generation attribute schema, so no attribute the increment logic classifies is silent; they are laser/lighting/temperature attributes.",
        "count": len(wnoevt),
        "names": wnoevt,
        "wnoclm_field_hash": WNOCLM,
        "wnoclm_count": len(wnoclm),
        "wnoclm_names": wnoclm,
    }
    out = os.path.join(di, "wnoevt_flags.json")
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(payload, f, ensure_ascii=False, indent=0, separators=(",", ":"))
        f.write("\n")
    print(f"written: {out}")


if __name__ == "__main__":
    main()
