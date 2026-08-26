use anyhow::{Context, bail};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Some(refno) = std::env::args().nth(1) else {
        bail!("usage: cata_resolve_probe DB_REFNO/ELEMENT_REFNO");
    };
    aios_core::init_surreal()
        .await
        .context("connect SurrealDB")?;
    let resolved =
        aios_database::fast_model::resolve::resolve_desi_comp(refno.as_str().into(), None)
            .await
            .context("resolve design component")?;
    for geometry in &resolved.geometries {
        if let Some(shape) = aios_core::prim_geo::category::convert_to_brep_shapes(geometry) {
            if shape.brep_shape.check_valid() {
                println!(
                    "SHAPE|refno={}|translation={:?}|rotation={:?}|hash={}",
                    shape.refno,
                    shape.transform.translation,
                    shape.transform.rotation,
                    shape.brep_shape.hash_unit_mesh_params()
                );
            }
        }
    }
    println!("{resolved:#?}");
    Ok(())
}
