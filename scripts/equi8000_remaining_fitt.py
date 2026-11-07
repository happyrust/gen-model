"""Real AMS8000 FITT increment and RVM evidence under its structural SCTN."""

import json
import struct
import sys
import time

import equi8000_live as e


TARGET = "24384/24949"
SCTN = "24384/24948"
FRMW = "24384/24947"
STRU = "24384/24946"


def source_session() -> int:
    data = e.SOURCE.read_bytes()
    page = struct.unpack_from(">I", data, 40)[0]
    return struct.unpack_from(">I", data, page * 2048 + 12)[0]


def inspect() -> None:
    roots = [SCTN, FRMW, STRU]
    payload = {
        "session": source_session(),
        "records": {root: e.api("model/records", {"generation_root": root}) for root in roots},
        "fitt": e.sql(
            "SELECT id,name,OWNER,SPRE,DESP,BANG,SHOP,BUIL FROM FITT:24384_24949;"
        )[0],
        "gen_roots": e.sql(
            "SELECT id,pe,noun,status,published_geometry_count,publication_status "
            "FROM gen_root WHERE pe IN [pe:24384_24946,pe:24384_24947,pe:24384_24948];"
        )[0],
    }
    print(json.dumps(payload, ensure_ascii=False, indent=2))


def edit(phase: str) -> None:
    folder = e.OUT / f"remaining-fitt-edit-{phase}"
    folder.mkdir(exist_ok=False)
    commands = {
        "rotate": [f"={TARGET}", "BANG 15", "SHOP TRUE", "BUIL TRUE"],
        "restore": [f"={TARGET}", "BANG 0", "SHOP FALSE", "BUIL FALSE"],
    }[phase]
    e.tty(
        folder,
        commands
        + [
            "Q REF",
            "Q BANG",
            "Q SPRE",
            "Q SHOP",
            "Q BUIL",
            f"SAVEWORK 'Remaining FITT {phase}'",
        ],
    )
    print((folder / "tty.log").read_text(encoding="utf-8-sig"))


def capture(label: str) -> None:
    folder = e.OUT / label
    folder.mkdir(exist_ok=False)
    target_session = source_session()
    deadline = time.monotonic() + 180
    while True:
        watermark = e.sql("SELECT applied_sesno FROM dbnum_watermark:8000;")[0][0]
        pending = e.sql("SELECT id FROM model_update_pending WHERE dbnum=8000;")
        if watermark.get("applied_sesno") == target_session and not pending[0]:
            break
        if time.monotonic() > deadline:
            raise TimeoutError((target_session, watermark, pending))
        time.sleep(2)
    e.save(folder / "records.json", e.api("model/records", {"generation_root": STRU}))
    e.save(folder / "sctn-records.json", e.api("model/records", {"generation_root": SCTN}))
    e.save(folder / "attributes.json", e.api("element/attributes", {"refno": TARGET}))
    e.save(
        folder / "database.json",
        e.sql(
            "SELECT * FROM FITT:24384_24949;"
            "SELECT * FROM dbnum_watermark:8000;"
            "SELECT * FROM gen_root WHERE pe=pe:24384_24946;"
        ),
    )
    e.tty(
        folder,
        [
            f"={SCTN}",
            "Q REF",
            "Q MEMB",
            "repre lev stru 6",
            "repre insu off",
            "repre obst off",
            "repre tube on",
            "export implied tube into separate containers",
            "export system /expdri.so",
            f'export file "{(folder / "model.rvm").as_posix()}"',
            "export holes on",
            "export autocolour displayexport on",
            "export repr on",
            "export CE",
            "export finish",
        ],
    )
    with (folder / "facets.log").open("w") as log:
        import subprocess

        subprocess.run(
            [
                "D:/work/plant-code/old/vendor/e3d-model/target/release/examples/rvm_facets_dump.exe",
                "--rvm",
                str(folder / "model.rvm"),
                "--contains",
                "",
                "--out",
                str(folder / "rvm.json"),
            ],
            stdout=log,
            stderr=subprocess.STDOUT,
            check=True,
        )
    inspect()


if __name__ == "__main__":
    action = sys.argv[1]
    if action == "inspect":
        inspect()
    elif action == "edit":
        edit(sys.argv[2])
    elif action == "capture":
        capture(sys.argv[2])
    else:
        raise ValueError(action)
