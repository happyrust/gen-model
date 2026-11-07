"""Reproduce the former AMS8000 COUP publication failure without ensure."""

import os
import struct
import subprocess
import sys
import time

import equi8000_live as e


ROOT = "24384/26642"
TARGET = "24384/26643"
ROOT_NAME = "/CODEX_TYPES_COUP_BRAN"
TARGET_NAME = "/CODEX_TYPES_COUP_SREV"

# The detached TTY path can currently fault in core.dll before processing the
# entry macro.  Attached launch still processes AVEVA_DESIGN_ENTRYMACRO and is
# sufficient for these single-purpose sessions.
os.environ.setdefault("L3_E3D_LAUNCH_MODE", "attached")
os.environ.setdefault("L3_E3D_CONSOLE_MODE", "show")
os.environ.setdefault("E3D_SKIP_REPAIR_MACRO", "1")


def source_session() -> int:
    data = e.SOURCE.read_bytes()
    page = struct.unpack_from(">I", data, 40)[0]
    return struct.unpack_from(">I", data, page * 2048 + 12)[0]


def edit(phase: str, label: str | None = None) -> None:
    folder = e.OUT / f"coup-publication-regression-edit-{label or phase}"
    folder.mkdir(exist_ok=False)
    command = {
        "oriented": "ORI Y IS N AND Z IS U",
        "unoriented": "ORIFLAG FALSE",
    }[phase]
    e.tty(
        folder,
        [
            TARGET_NAME,
            command,
            "Q REF",
            "Q ATT",
            f"SAVEWORK 'COUP automatic publication regression {phase}'",
        ],
    )
    print((folder / "tty.log").read_text(encoding="utf-8-sig"))


def capture(label: str) -> None:
    folder = e.OUT / label
    folder.mkdir(exist_ok=False)
    target_session = source_session()
    deadline = time.monotonic() + 180
    while True:
        # 模型欠账由双水位回答（ADR-072 D5 之后没有队列表可数）：任务收尾把 model_sesno
        # 盖到 applied_sesno 才算这一版的模型落完。
        watermark = e.sql("SELECT applied_sesno, model_sesno FROM dbnum_watermark:8000;")[0][0]
        settled = watermark.get("model_sesno") == watermark.get("applied_sesno")
        records = e.api("model/records", {"generation_root": ROOT})
        vector_session = next(
            row["sesno"] for row in records["session_vector"] if row["dbnum"] == 8000
        )
        if watermark.get("applied_sesno") == target_session and settled and vector_session == target_session:
            break
        if time.monotonic() > deadline:
            e.save(
                folder / "publication-timeout.json",
                {
                    "target_session": target_session,
                    "watermark": watermark,
                    "records": records,
                },
            )
            raise TimeoutError((target_session, watermark, vector_session))
        time.sleep(2)

    e.save(folder / "records.json", records)
    e.save(folder / "attributes.json", e.api("element/attributes", {"refno": TARGET}))
    e.save(
        folder / "database.json",
        e.sql(
            "SELECT * FROM COUP:24384_26643;"
            "SELECT * FROM BRAN:24384_26642;"
            "SELECT * FROM dbnum_watermark:8000;"
            "SELECT * FROM gen_root:24384_26642;"
        ),
    )
    e.tty(
        folder,
        [
            ROOT_NAME,
            "Q REF",
            "Q MEMB",
            "repre lev 6",
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
    print({"label": label, "session": target_session, "records": records["total"]})


if __name__ == "__main__":
    action = sys.argv[1]
    if action == "edit":
        edit(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None)
    elif action == "capture":
        capture(sys.argv[2])
    else:
        raise ValueError(action)
