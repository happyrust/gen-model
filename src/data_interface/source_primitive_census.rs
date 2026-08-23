//! Read-only census of geometry nouns directly from dabacon source files.
//!
//! This deliberately reads attributes before `inst_geo` normalization so fields such as
//! `YOFF` cannot disappear before the census observes them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aios_core::NamedAttrMap;
#[cfg(feature = "manifold")]
use aios_core::RefU64;
use aios_core::tool::db_tool::db1_hash_i32;
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_TARGET_NOUNS: &[&str] = &["SPHE", "SLCY", "POHE", "POLYHE", "SNOU", "NSNO"];

const DIMENSION_ATTRIBUTES: &[&str] = &[
    "RADI", "DIAM", "HEIG", "DTOP", "DBOT", "XOFF", "YOFF", "XTOP", "YTOP", "XBOT", "YBOT", "ANGL",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourcePrimitiveSample {
    pub source_file: PathBuf,
    pub source_sha256: String,
    pub dbnum: u32,
    pub refno: String,
    pub owner: String,
    pub noun: String,
    pub dimensions: BTreeMap<String, f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh_validation: Option<SourceMeshValidation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceMeshValidation {
    pub vertices: usize,
    pub triangles: usize,
    pub signed_volume: f64,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceFileCensus {
    pub source_file: PathBuf,
    pub source_sha256: String,
    pub dbnum: u32,
    pub indexed_elements: usize,
    pub noun_counts: BTreeMap<String, usize>,
    pub samples: Vec<SourcePrimitiveSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourcePrimitiveCensus {
    pub root: PathBuf,
    pub target_nouns: Vec<String>,
    pub files_scanned: usize,
    pub indexed_elements: usize,
    pub noun_counts: BTreeMap<String, usize>,
    pub samples: Vec<SourcePrimitiveSample>,
}

fn selected_dimensions(attrs: &NamedAttrMap) -> BTreeMap<String, f32> {
    DIMENSION_ATTRIBUTES
        .iter()
        .filter_map(|name| {
            attrs
                .get_f32(name)
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}

fn target_hashes(target_nouns: &[String]) -> HashMap<i32, String> {
    target_nouns
        .iter()
        .map(|noun| {
            let noun = noun.trim().to_ascii_uppercase();
            (db1_hash_i32(&noun), noun)
        })
        .collect()
}

#[cfg(feature = "manifold")]
fn validate_brep_shape(
    noun: &str,
    shape: Box<dyn aios_core::shape::pdms_shape::BrepShapeTrait>,
) -> anyhow::Result<SourceMeshValidation> {
    use aios_core::shape::pdms_shape::VerifiedShape;

    ensure!(
        shape.check_valid(),
        "source noun {noun} produced invalid geometry parameters"
    );
    let param = shape
        .convert_to_geo_param()
        .with_context(|| format!("source noun {noun} did not convert to PdmsGeoParam"))?
        .convert_to_unit_param();
    let mesh = crate::fast_model::manifold_tessellate::tessellate_libgm_param(&param)?
        .with_context(|| format!("source noun {noun} was classified as a non-shape"))?;
    ensure!(
        !mesh.vertices.is_empty(),
        "source noun {noun} generated no vertices"
    );
    ensure!(
        !mesh.indices.is_empty() && mesh.indices.len() % 3 == 0,
        "source noun {noun} generated an invalid triangle index count"
    );
    ensure!(
        mesh.indices
            .iter()
            .all(|index| (*index as usize) < mesh.vertices.len()),
        "source noun {noun} generated an out-of-range index"
    );
    ensure!(
        mesh.vertices.iter().all(|point| point.is_finite())
            && mesh.normals.iter().all(|normal| normal.is_finite()),
        "source noun {noun} generated non-finite mesh data"
    );
    ensure!(
        mesh.normals.len() == mesh.vertices.len(),
        "source noun {noun} generated {} vertices but {} normals",
        mesh.vertices.len(),
        mesh.normals.len()
    );

    let mut aabb_min = glam::Vec3::splat(f32::INFINITY);
    let mut aabb_max = glam::Vec3::splat(f32::NEG_INFINITY);
    for point in &mesh.vertices {
        aabb_min = aabb_min.min(*point);
        aabb_max = aabb_max.max(*point);
    }
    ensure!(
        (aabb_max - aabb_min).min_element() > 0.0,
        "source noun {noun} generated a degenerate AABB"
    );
    let signed_volume = mesh
        .indices
        .chunks_exact(3)
        .map(|triangle| {
            let a = mesh.vertices[triangle[0] as usize].as_dvec3();
            let b = mesh.vertices[triangle[1] as usize].as_dvec3();
            let c = mesh.vertices[triangle[2] as usize].as_dvec3();
            a.dot(b.cross(c)) / 6.0
        })
        .sum::<f64>();
    ensure!(
        signed_volume.abs() > 1e-9,
        "source noun {noun} generated zero signed volume"
    );
    Ok(SourceMeshValidation {
        vertices: mesh.vertices.len(),
        triangles: mesh.indices.len() / 3,
        signed_volume,
        aabb_min: aabb_min.to_array(),
        aabb_max: aabb_max.to_array(),
    })
}

#[cfg(feature = "manifold")]
fn validate_source_mesh(noun: &str, attrs: &NamedAttrMap) -> anyhow::Result<SourceMeshValidation> {
    let shape = attrs
        .create_brep_shape(None)
        .with_context(|| format!("source noun {noun} did not create a BrepShapeTrait"))?;
    validate_brep_shape(noun, shape)
}

#[cfg(feature = "manifold")]
fn parse_source_attrs(
    data: &aios_core::db::DbBasicData,
    refno: RefU64,
) -> anyhow::Result<NamedAttrMap> {
    let entry = data
        .refno_table_map
        .get(&refno)
        .with_context(|| format!("source closure is missing refno {}", refno.to_pe_key()))?;
    ensure!(
        entry.pos >= 4 && entry.pos - 4 < data.bytes.len(),
        "invalid closure element position {} for {}",
        entry.pos,
        refno.to_pe_key()
    );
    let element = parse_pdms_db::parse::parse_raw_ele_data_with_info(
        &data.bytes[entry.pos - 4..],
        &aios_core::get_default_pdms_db_info(),
    )?;
    Ok(element.whole_attmap.merge())
}

#[cfg(feature = "manifold")]
fn source_descendants(
    data: &aios_core::db::DbBasicData,
    root: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    let mut pending = data.children_map.get(&root).cloned().unwrap_or_default();
    let mut descendants = Vec::new();
    let mut visited = HashSet::new();
    while let Some(refno) = pending.pop() {
        ensure!(
            visited.insert(refno),
            "source closure below {} contains a repeated member {}",
            root.to_pe_key(),
            refno.to_pe_key()
        );
        descendants.push(refno);
        if let Some(children) = data.children_map.get(&refno) {
            pending.extend(children.iter().copied());
        }
    }
    Ok(descendants)
}

#[cfg(feature = "manifold")]
fn validate_source_polyhedron(
    noun: &str,
    root: RefU64,
    data: &aios_core::db::DbBasicData,
) -> anyhow::Result<SourceMeshValidation> {
    use aios_core::prim_geo::polyhedron::{Polygon, Polyhedron};

    let direct_children = data
        .children_map
        .get(&root)
        .with_context(|| format!("source {noun} {} has no child closure", root.to_pe_key()))?;
    ensure!(
        !direct_children.is_empty(),
        "source {noun} {} has an empty child closure",
        root.to_pe_key()
    );
    let first_type = parse_source_attrs(data, direct_children[0])?
        .get_type_str()
        .to_string();
    let (polygons, is_polyhe) = if first_type == "POLPTL" {
        let mut vertices = HashMap::new();
        for refno in data
            .children_map
            .get(&direct_children[0])
            .cloned()
            .unwrap_or_default()
        {
            let attrs = parse_source_attrs(data, refno)?;
            if attrs.get_type_str() == "POIN" {
                vertices.insert(
                    refno,
                    attrs
                        .get_position()
                        .with_context(|| format!("POIN {} has no position", refno.to_pe_key()))?,
                );
            }
        }
        ensure!(
            !vertices.is_empty(),
            "source {noun} {} POLPTL has no POIN vertices",
            root.to_pe_key()
        );

        let mut index_runs: HashMap<RefU64, Vec<RefU64>> = HashMap::new();
        let mut polygon_loops: HashMap<RefU64, Vec<RefU64>> = HashMap::new();
        for refno in source_descendants(data, root)? {
            let attrs = parse_source_attrs(data, refno)?;
            match attrs.get_type_str() {
                "LOOPTS" => {
                    let refs = attrs
                        .get_refno_vec("VXREF")
                        .with_context(|| format!("LOOPTS {} has no VXREF", refno.to_pe_key()))?;
                    index_runs
                        .entry(attrs.get_owner().into())
                        .or_default()
                        .extend(refs.into_iter().map(|refno| -> RefU64 { refno.into() }));
                }
                "POLOOP" => {
                    polygon_loops
                        .entry(attrs.get_owner().into())
                        .or_default()
                        .push(refno);
                }
                _ => {}
            }
        }
        let mut polygons = Vec::new();
        for loop_refnos in polygon_loops.into_values() {
            let mut loops = Vec::new();
            for loop_refno in loop_refnos {
                let vertex_refnos = index_runs.get(&loop_refno).with_context(|| {
                    format!("POLOOP {} has no LOOPTS indices", loop_refno.to_pe_key())
                })?;
                ensure!(
                    vertex_refnos.len() >= 3,
                    "POLOOP {} has fewer than three indices",
                    loop_refno.to_pe_key()
                );
                let mut points = Vec::with_capacity(vertex_refnos.len());
                for vertex_refno in vertex_refnos {
                    points.push(*vertices.get(vertex_refno).with_context(|| {
                        format!(
                            "POLOOP {} references missing POIN {}",
                            loop_refno.to_pe_key(),
                            vertex_refno.to_pe_key()
                        )
                    })?);
                }
                loops.push(points);
            }
            polygons.push(Polygon { loops });
        }
        (polygons, true)
    } else {
        let mut polygons = Vec::new();
        for face_refno in direct_children {
            let child_refnos = data
                .children_map
                .get(face_refno)
                .cloned()
                .unwrap_or_default();
            let mut points = Vec::new();
            for vertex_refno in child_refnos {
                points.push(
                    parse_source_attrs(data, vertex_refno)?
                        .get_position()
                        .with_context(|| {
                            format!("vertex {} has no position", vertex_refno.to_pe_key())
                        })?,
                );
            }
            ensure!(
                points.len() >= 3,
                "source {noun} face {} has fewer than three vertices",
                face_refno.to_pe_key()
            );
            polygons.push(Polygon {
                loops: vec![points],
            });
        }
        (polygons, false)
    };
    ensure!(
        !polygons.is_empty(),
        "source {noun} {} assembled no polygons",
        root.to_pe_key()
    );
    validate_brep_shape(
        noun,
        Box::new(Polyhedron {
            polygons,
            mesh: None,
            is_polyhe,
        }),
    )
}

pub fn census_source_file(
    path: &Path,
    target_nouns: &[String],
    validate_meshes: bool,
) -> anyhow::Result<SourceFileCensus> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("read dabacon source file {}", path.display()))?;
    let source_sha256 = hex::encode(Sha256::digest(&bytes));
    let basic_info = parse_pdms_db::parse::parse_file_basic_info(&bytes);
    let index = parse_pdms_db::parse::parse_db_index_data(bytes);
    let db_info = aios_core::get_default_pdms_db_info();
    let targets = target_hashes(target_nouns);
    #[cfg(feature = "manifold")]
    let polyhedron_closure = if validate_meshes
        && index.refno_table_map.iter().any(|entry| {
            targets
                .get(&entry.noun_hash)
                .is_some_and(|noun| matches!(noun.as_str(), "POHE" | "POLYHE"))
        }) {
        Some(parse_pdms_db::parse::parse_db_basic_data(
            index.bytes.clone(),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
            "occ-retire-census",
        )?)
    } else {
        None
    };
    let mut noun_counts = BTreeMap::new();
    let mut samples = Vec::new();

    for entry in index.refno_table_map.iter() {
        let Some(noun) = targets.get(&entry.noun_hash) else {
            continue;
        };
        *noun_counts.entry(noun.clone()).or_insert(0) += 1;
        ensure!(
            entry.pos >= 4 && entry.pos - 4 < index.bytes.len(),
            "invalid element position {} in {}",
            entry.pos,
            path.display()
        );
        let element = parse_pdms_db::parse::parse_raw_ele_data_with_info(
            &index.bytes[entry.pos - 4..],
            &db_info,
        )
        .with_context(|| {
            format!(
                "parse target noun {noun} refno={} from {}",
                entry.key().to_pe_key(),
                path.display()
            )
        })?;
        let attrs = element.whole_attmap.merge();
        #[cfg(feature = "manifold")]
        let mesh_validation = if validate_meshes {
            Some(if matches!(noun.as_str(), "POHE" | "POLYHE") {
                validate_source_polyhedron(
                    noun,
                    *entry.key(),
                    polyhedron_closure
                        .as_ref()
                        .context("polyhedron closure was not parsed")?,
                )?
            } else {
                validate_source_mesh(noun, &attrs)?
            })
        } else {
            None
        };
        #[cfg(not(feature = "manifold"))]
        let mesh_validation = {
            ensure!(
                !validate_meshes,
                "mesh validation requires the `manifold` feature"
            );
            None
        };
        samples.push(SourcePrimitiveSample {
            source_file: path.to_path_buf(),
            source_sha256: source_sha256.clone(),
            dbnum: basic_info.db_no,
            refno: entry.key().to_pe_key(),
            owner: element.owner.to_pe_key(),
            noun: noun.clone(),
            dimensions: selected_dimensions(&attrs),
            mesh_validation,
        });
    }
    samples.sort_by(|left, right| {
        (left.noun.as_str(), left.refno.as_str()).cmp(&(right.noun.as_str(), right.refno.as_str()))
    });

    Ok(SourceFileCensus {
        source_file: path.to_path_buf(),
        source_sha256,
        dbnum: basic_info.db_no,
        indexed_elements: index.refno_table_map.len(),
        noun_counts,
        samples,
    })
}

pub fn census_source_root(
    root: &Path,
    target_nouns: &[String],
    validate_meshes: bool,
) -> anyhow::Result<SourcePrimitiveCensus> {
    let mut files = std::fs::read_dir(root)
        .with_context(|| format!("read dabacon source directory {}", root.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("_0001"))
        })
        .collect::<Vec<_>>();
    files.sort();
    ensure!(
        !files.is_empty(),
        "no dabacon *_0001 files under {}",
        root.display()
    );

    let mut result = SourcePrimitiveCensus {
        root: root.to_path_buf(),
        target_nouns: target_nouns
            .iter()
            .map(|noun| noun.trim().to_ascii_uppercase())
            .collect(),
        files_scanned: 0,
        indexed_elements: 0,
        noun_counts: BTreeMap::new(),
        samples: Vec::new(),
    };
    for path in files {
        let file = census_source_file(&path, target_nouns, validate_meshes)?;
        result.files_scanned += 1;
        result.indexed_elements += file.indexed_elements;
        for (noun, count) in file.noun_counts {
            *result.noun_counts.entry(noun).or_insert(0) += count;
        }
        result.samples.extend(file.samples);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::NamedAttrValue;

    #[test]
    fn target_hashes_normalize_case_and_whitespace() {
        let targets = target_hashes(&[" sphe ".to_string(), "SnOu".to_string()]);
        assert_eq!(
            targets.get(&db1_hash_i32("SPHE")).map(String::as_str),
            Some("SPHE")
        );
        assert_eq!(
            targets.get(&db1_hash_i32("SNOU")).map(String::as_str),
            Some("SNOU")
        );
    }

    #[test]
    fn selected_dimensions_preserve_zero_yoff_and_drop_unrelated_fields() {
        let mut attrs = NamedAttrMap::default();
        attrs
            .map
            .insert("XOFF".to_string(), NamedAttrValue::F32Type(12.5));
        attrs
            .map
            .insert("YOFF".to_string(), NamedAttrValue::F32Type(0.0));
        attrs.map.insert(
            "NAME".to_string(),
            NamedAttrValue::StringType("sample".into()),
        );
        let dimensions = selected_dimensions(&attrs);
        assert_eq!(dimensions.get("XOFF"), Some(&12.5));
        assert_eq!(dimensions.get("YOFF"), Some(&0.0));
        assert!(!dimensions.contains_key("NAME"));
    }

    #[cfg(feature = "manifold")]
    #[test]
    fn nonzero_yoff_snout_from_source_attributes_generates_a_closed_mesh() {
        let mut attrs = NamedAttrMap::default();
        for (name, value) in [
            ("HEIG", 1200.0),
            ("DBOT", 2000.0),
            ("DTOP", 700.0),
            ("XOFF", 0.0),
            ("YOFF", 650.0),
        ] {
            attrs
                .map
                .insert(name.to_string(), NamedAttrValue::F32Type(value));
        }
        attrs.map.insert(
            "TYPE".to_string(),
            NamedAttrValue::StringType("SNOU".into()),
        );
        let validation = validate_source_mesh("SNOU", &attrs).expect("YOFF snout must tessellate");
        assert!(validation.vertices > 16);
        assert!(validation.triangles > 16);
        assert!(validation.signed_volume.abs() > 0.1);
    }
}
