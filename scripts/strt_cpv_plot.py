"""Render the three reported STRTs from generated OBJ and native RVM records."""
from pathlib import Path
import json
import re
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from mpl_toolkits.mplot3d.art3d import Poly3DCollection

OUT = Path(__file__).resolve().parents[1] / "output/strt-cpv-20260905"
REFS = {"24381/76730", "24381/76731", "24381/76732"}


def obj_faces(path):
    vertices, faces = {}, []
    selected, index = False, 0
    with path.open() as source:
        for line in source:
            if line.startswith("o "):
                selected = any(ref in line for ref in REFS)
            elif line.startswith("v "):
                index += 1
                if selected:
                    vertices[index] = [float(v) for v in line.split()[1:4]]
            elif selected and line.startswith("f "):
                faces.append([vertices[int(v.split("/")[0])] for v in line.split()[1:]])
    assert faces
    return [np.array(face) for face in faces]


def native_faces():
    faces = []
    for group in json.loads((OUT / "branch-hvac6.json").read_text()):
        for geometry in group["geometries"]:
            assert geometry["kind"] == "Cylinder"
            radius, height = map(float, re.search(r"radius: ([\d.eE+-]+), height: ([\d.eE+-]+)", geometry["params"]).groups())
            transform = np.array(geometry["transform_cols_m"]).reshape(4, 3) * 1000
            angle = np.linspace(0, 2*np.pi, 65)[:-1]
            ring = np.c_[radius*np.cos(angle), radius*np.sin(angle), np.zeros(64)]
            bottom, top = ring.copy(), ring.copy()
            bottom[:, 2] = -height/2
            top[:, 2] = height/2
            for polygon in [bottom[::-1], top] + [np.array([bottom[i], bottom[(i+1)%64], top[(i+1)%64], top[i]]) for i in range(64)]:
                faces.append(np.c_[polygon, np.ones(len(polygon))] @ transform)
    return faces


panels = [obj_faces(OUT / "before-all/model.obj"), obj_faces(OUT / "after-branch/model.obj"), native_faces()]
origin = np.array([6484.353558938357, -8304.896688652032, -695.0])
fig = plt.figure(figsize=(15, 7), facecolor="#f4f7fb")
for index, (faces, title, color) in enumerate(zip(panels, ["Before: incorrect 90-degree rotation", "Fixed geometry", "E3D TTY / RVM reference"], ["#bc6250", "#4383b7", "#459782"]), 1):
    ax = fig.add_subplot(1, 3, index, projection="3d")
    ax.set_facecolor("#f4f7fb")
    local = [(face-origin)/1000 for face in faces]
    ax.add_collection3d(Poly3DCollection(local, facecolor=color, edgecolor="#244052", linewidth=.09, alpha=.92))
    ax.set(xlim=(-1.7, 1.1), ylim=(-1.3, 1.5), zlim=(-1, 4.6), xlabel="X (m)", ylabel="Y (m)", zlabel="Z (m)")
    ax.set_box_aspect((2.8, 2.8, 5.6))
    ax.view_init(elev=18, azim=-55)
    ax.set_title(title, fontsize=12, pad=15)
fig.suptitle("CPV branch 24381/76729 | STRT 76730, 76731, 76732 | HVAC level 6", fontsize=15)
fig.text(.5, .025, "Same camera and scale. Three 1,220 mm duct sections; body and four flange cylinders per section.", ha="center", fontsize=11)
fig.tight_layout(rect=(0, .13, 1, .94))
fig.savefig(OUT / "comparison.png", dpi=140)
print(OUT / "comparison.png")
