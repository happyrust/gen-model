//! 网格体检断言，供 `mesh_primitives` / `sweep_mesh` 等自建网格生成器的单测共用。
//!
//! 判据只有一条：生成出来的东西必须是**能拿去做布尔的闭合实体**。
//! 「非空」证明不了这件事——绕向反了、少一块端盖、法线是零向量，网格照样非空，
//! 但 manifold 那边会给出静默错误的结果。

use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use std::collections::HashMap;

/// 结构体检：非空、索引不越界、法线齐备且是单位向量、没有零面积三角、
/// 焊接后是闭合可定向流形、按散度定理算出的体积为正（即三角朝外）。
pub fn assert_solid_mesh(mesh: &PlantMesh, label: &str) {
    assert!(
        mesh.indices.len() >= 3 && mesh.indices.len() % 3 == 0,
        "{label}: 索引数 {} 不是正的 3 的倍数",
        mesh.indices.len()
    );
    assert!(
        mesh.vertices.len() >= 3,
        "{label}: 顶点数只有 {}",
        mesh.vertices.len()
    );
    for &idx in &mesh.indices {
        assert!(
            (idx as usize) < mesh.vertices.len(),
            "{label}: 索引 {idx} 越界（顶点数 {}）",
            mesh.vertices.len()
        );
    }
    assert_eq!(
        mesh.normals.len(),
        mesh.vertices.len(),
        "{label}: 法线数与顶点数不一致"
    );
    for (i, n) in mesh.normals.iter().enumerate() {
        let len = n.length();
        assert!(
            (len - 1.0).abs() < 1e-3,
            "{label}: 第 {i} 个法线长度 {len}，不是单位向量"
        );
    }

    let (min, max) = mesh_bounds(mesh);
    let diag = (max - min).length();
    assert!(diag > 0.0, "{label}: 包围盒退化成一点");
    if let Some(aabb) = mesh.aabb {
        let tol = diag * 1e-5;
        for (axis, (lo, hi), (alo, ahi)) in [
            ("x", (min.x, max.x), (aabb.mins.x, aabb.maxs.x)),
            ("y", (min.y, max.y), (aabb.mins.y, aabb.maxs.y)),
            ("z", (min.z, max.z), (aabb.mins.z, aabb.maxs.z)),
        ] {
            assert!(
                (lo - alo).abs() <= tol && (hi - ahi).abs() <= tol,
                "{label}: 随附 AABB 的 {axis} 轴 [{alo}, {ahi}] 与顶点实际 [{lo}, {hi}] 不符"
            );
        }
    } else {
        panic!("{label}: 缺少随附 AABB");
    }

    // 面积阈值取包围盒尺度的相对量，避免大模型上被绝对值卡住
    let min_cross = diag * diag * 1e-9;
    for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
        let (p0, p1, p2) = (
            mesh.vertices[tri[0] as usize],
            mesh.vertices[tri[1] as usize],
            mesh.vertices[tri[2] as usize],
        );
        let cross = (p1 - p0).cross(p2 - p0).length();
        assert!(
            cross > min_cross,
            "{label}: 第 {t} 个三角退化（叉积长度 {cross} <= {min_cross}）"
        );
    }

    assert_closed_manifold(mesh, diag, label);

    let volume = mesh_volume(mesh);
    assert!(
        volume > 0.0,
        "{label}: 有向体积 {volume} 非正，三角绕向朝内"
    );
}

pub fn mesh_bounds(mesh: &PlantMesh) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in &mesh.vertices {
        min = min.min(*v);
        max = max.max(*v);
    }
    (min, max)
}

/// 散度定理：闭合面上 Σ det(v0, v1, v2) / 6 就是围出的有向体积。
pub fn mesh_volume(mesh: &PlantMesh) -> f32 {
    let mut acc = 0.0f64;
    for tri in mesh.indices.chunks_exact(3) {
        let p = |i: usize| {
            let v = mesh.vertices[tri[i] as usize];
            (v.x as f64, v.y as f64, v.z as f64)
        };
        let (ax, ay, az) = p(0);
        let (bx, by, bz) = p(1);
        let (cx, cy, cz) = p(2);
        acc +=
            (ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx)) / 6.0;
    }
    acc as f32
}

/// 位置相同的顶点合并成一个代表点：缝合线与 cap 都是刻意复制顶点做出来的，
/// 不先焊接就无法做拓扑判定。
fn weld(vertices: &[Vec3], tol: f32) -> Vec<u32> {
    let cell = tol.max(f32::MIN_POSITIVE);
    let mut buckets: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut remap = vec![0u32; vertices.len()];
    for (i, v) in vertices.iter().enumerate() {
        let key = [
            (v.x / cell).floor() as i64,
            (v.y / cell).floor() as i64,
            (v.z / cell).floor() as i64,
        ];
        let mut hit = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(bucket) = buckets.get(&[key[0] + dx, key[1] + dy, key[2] + dz]) else {
                        continue;
                    };
                    for &c in bucket {
                        if vertices[c as usize].distance(*v) <= tol {
                            hit = Some(c);
                            break 'search;
                        }
                    }
                }
            }
        }
        match hit {
            Some(c) => remap[i] = c,
            None => {
                remap[i] = i as u32;
                buckets.entry(key).or_default().push(i as u32);
            }
        }
    }
    remap
}

/// 闭合可定向：每条有向边恰好出现一次，且它的反向边也恰好出现一次。
/// 出现两次 = 绕向不一致或面重叠；反向缺失 = 有洞。
pub fn assert_closed_manifold(mesh: &PlantMesh, diag: f32, label: &str) {
    let remap = weld(&mesh.vertices, (diag * 1e-5).max(1e-6));
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for tri in mesh.indices.chunks_exact(3) {
        let a = remap[tri[0] as usize];
        let b = remap[tri[1] as usize];
        let c = remap[tri[2] as usize];
        assert!(
            a != b && b != c && c != a,
            "{label}: 焊接后三角 ({a},{b},{c}) 退化，说明有重合顶点被当成不同点用"
        );
        for edge in [(a, b), (b, c), (c, a)] {
            *edges.entry(edge).or_insert(0) += 1;
        }
    }
    for (&(u, v), &n) in &edges {
        assert_eq!(n, 1, "{label}: 有向边 ({u},{v}) 出现 {n} 次，绕向不一致");
        let back = edges.get(&(v, u)).copied().unwrap_or(0);
        assert_eq!(
            back, 1,
            "{label}: 边 ({u},{v}) 的反向边出现 {back} 次，网格有洞"
        );
    }
}

pub fn assert_bounds(mesh: &PlantMesh, expect_min: Vec3, expect_max: Vec3, label: &str) {
    let tol = (expect_max - expect_min).length() * 1e-4;
    assert_bounds_tol(mesh, expect_min, expect_max, tol, label);
}

/// 曲面被弦割掉一点的地方（极值不落在采样点上）要放宽容差。
pub fn assert_bounds_tol(
    mesh: &PlantMesh,
    expect_min: Vec3,
    expect_max: Vec3,
    tol: f32,
    label: &str,
) {
    let (min, max) = mesh_bounds(mesh);
    assert!(
        min.abs_diff_eq(expect_min, tol) && max.abs_diff_eq(expect_max, tol),
        "{label}: 包围盒 [{min}, {max}] 与期望 [{expect_min}, {expect_max}] 不符（容差 {tol}）"
    );
}

/// 与解析体积对拍。离散网格内接于真实曲面，所以允许偏小；`rel_tol` 按分段数给。
pub fn assert_volume(mesh: &PlantMesh, exact: f32, rel_tol: f32, label: &str) {
    let v = mesh_volume(mesh);
    assert!(
        (v - exact).abs() <= exact.abs() * rel_tol,
        "{label}: 体积 {v} 与解析值 {exact} 相差超过 {:.1}%",
        rel_tol * 100.0
    );
}
