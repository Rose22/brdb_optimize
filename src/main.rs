/*
 * takes a brdb world file and optimizes it by:
 * - freezing all wheels and spheres
 * - TODO: freezing all entities not attached to any kind of joint (bearing/slider)
 * - TODO: freezing all physics grids that contain an engine (so basically, a vehicle)
 * - disabling castshadows on all lights everywhere
 * - forcing radius and brightness of all lights down to a reasonable limit
 * - TODO: stripping revisions to only the last 600 (keeps filesize small)
 *     (600 revisions = roughly 2 days assuming 5 minute autosave interval)
 * - TODO: delete stray weight components on the main grid
 */

use std::{
    env,
    process,
    path::PathBuf
};
use brdb::{
    AsBrdbValue, Brdb, BrdbComponent, EntityChunkSoA, IntoReader, pending::BrPendingFs, schema::BrdbValue,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).take(1).collect();
    
    if args.is_empty() {
        println!("You must run the program with an argument that points to a world file.");
        process::exit(1);
    }

    println!("Reading file {:?}", args[0]);
    let src = PathBuf::from(&args[0]);
    let stem = src.file_stem().unwrap().to_string_lossy();
    let mut dst = src.with_file_name(format!("{stem}.optimized.brdb"));

    assert!(src.exists());

    let db = Brdb::open(src)?.into_reader();

    let global_data = db.global_data()?;
    let entity_schema = db.entities_schema()?;
    let component_schema = db.components_schema()?;

    let mut num_entities_modified: u32 = 0;
    let mut num_components_modified: u32 = 0;

    // ------------------
    // Freeze all entities that are known to cause lag
    // ------------------
    println!("---SEP---");
    println!("freezing entities..");

    let mut entity_chunk_files = vec![];
    for index in db.entity_chunk_index()? {
        let entities = db.entity_chunk(index)?;

        let mut soa = EntityChunkSoA::default();
        for mut e in entities.into_iter() {
            let ent_type = e.data.get_schema_struct().unwrap().0;

            if ent_type.starts_with("Entity_Wheel") || ent_type.starts_with("Entity_Ball") {
                if !e.frozen {
                    println!("[entity:{}] freezing {ent_type}..", e.id.unwrap());
                    e.frozen = true;
                    num_entities_modified += 1;
                }
            } else {
                /*
                // unfreeze all entities
                println!("[entity:{}] unfreezing {ent_type}", e.id.unwrap());
                e.frozen = false;
                */
            }

            soa.add_entity(&global_data, &e, e.id.unwrap() as u32);
        }

        entity_chunk_files.push((
            format!("{index}.mps"),
            BrPendingFs::File(Some(soa.to_bytes(&entity_schema)?)),
        ));
    }

    let wheels_patch = BrPendingFs::Root(vec![(
        "World".to_owned(),
        BrPendingFs::Folder(Some(vec![(
            "0".to_string(),
            BrPendingFs::Folder(Some(vec![(
                "Entities".to_string(),
                BrPendingFs::Folder(Some(vec![(
                    "Chunks".to_string(),
                    BrPendingFs::Folder(Some(entity_chunk_files)),
                )])),
            )])),
        )])),
    )]);

    // ------------------
    // Optimize components
    // ------------------
    println!("---SEP---");
    println!("optimizing components..");
    let mut grid_ids = vec![1];

    // Collect dynamic brick grid IDs
    for chunk in db.entity_chunk_index()? {
        for entity in db.entity_chunk(chunk)? {
            if entity.data
                .get_schema_struct()
                .is_some_and(|s| s.0.as_ref() == "Entity_DynamicBrickGrid")
            {
                if let Some(id) = entity.id {
                    grid_ids.push(id);
                }
            }
        }
    }

    let mut grids_files = vec![];

    for grid in &grid_ids {
        let chunks = db.brick_chunk_index(*grid)?;
        let mut chunk_files = vec![];
        let mut num_grid_modified = 0;

        for chunk in chunks {
            let mut num_chunk_modified = 0;

            if chunk.num_components == 0 {
                continue;
            }

            let (mut soa, components) = db.component_chunk(*grid, *chunk)?;

            for mut component in components {
                let component_name = String::from(component.get_name());

                /*
                if *grid == 1 {
                    // main grid
                    if component_name == "BrickComponentData_WeightBrick" {
                        // neutralize weight components on the main grid
                        if component.prop("Mass")?.as_brdb_f32()? > 0.0 {
                            println!("[grid:{grid}] weight: disabling..");
                            component.set_prop("Mass", BrdbValue::F32(0.0));

                            modified = true;
                            num_components_modified += 1;
                        }
                    }
                }
                */

                //if component.prop("bCastShadows").is_ok()
                if
                    component_name == "BrickComponentData_PointLight"
                    ||
                    component_name == "BrickComponentData_SpotLight"
                {
                    // light component
                    let mut modified: bool = false;

                    /*
                    println!(
                        "grid {grid} chunk {} mutating component {}",
                        *chunk,
                        component.get_name()
                    );
                    */

                    // force light radius down to 500
                    let component_radius = component.prop("Radius")?.as_brdb_f32()?;
                    if component_radius > 5000.0 {
                        // for some reason the game stores radiuses as thousands..
                        println!("[grid:{grid}] light: radius exceeds 500, forcing down..");
                        component.set_prop("Radius", BrdbValue::F32(5000.0));

                        modified = true;
                    }
                    // force light brightness down to 500
                    let component_brightness = component.prop("Brightness")?.as_brdb_f32()?;
                    if component_brightness > 400.0 {
                        println!("[grid:{grid}] light: brightness exceeds 400, forcing down..");
                        component.set_prop("Brightness", BrdbValue::F32(400.0));

                        modified = true;
                    }

                    let component_cast_shadows = component.prop("bCastShadows")?.as_brdb_bool()?;
                    if component_cast_shadows {
                        println!("[grid:{grid}] light: disabling cast shadows..");
                        component.set_prop("bCastShadows", BrdbValue::Bool(false))?;

                        modified = true;
                    }

                    if modified {
                        num_grid_modified += 1;
                        num_chunk_modified += 1;
                        num_components_modified += 1;
                    }
                }

                soa.unwritten_struct_data.push(Box::new(component));
            }

            if num_chunk_modified > 0 {
                chunk_files.push((
                    format!("{}.mps", *chunk),
                    BrPendingFs::File(Some(soa.to_bytes(&component_schema)?)),
                ));
            }
        }

        if num_grid_modified > 0 {
            println!(
                "[grid:{grid}] {num_grid_modified} components optimized"
            );
            grids_files.push((
                grid.to_string(),
                BrPendingFs::Folder(Some(vec![(
                    "Components".to_string(),
                    BrPendingFs::Folder(Some(chunk_files)),
                )])),
            ));
        }
    }

    println!("---SEP---");

    let shadows_patch = BrPendingFs::Root(vec![(
        "World".to_owned(),
        BrPendingFs::Folder(Some(vec![(
            "0".to_string(),
            BrPendingFs::Folder(Some(vec![(
                "Bricks".to_string(),
                BrPendingFs::Folder(Some(vec![(
                    "Grids".to_string(),
                    BrPendingFs::Folder(Some(grids_files)),
                )])),
            )])),
        )])),
    )]);


    /* 
    println!("stripping revisions..");
    db.conn.execute(
    */

    println!();
    println!("optimized {num_entities_modified} entities and {num_components_modified} components!");
    println!("writing to world file..");

    // ------------------
    // Write combined patch
    // ------------------
    let pending = db
        .to_pending()?
        .with_patch(wheels_patch)?
        .with_patch(shadows_patch)?;

    if dst.exists() {
        std::fs::remove_file(&dst)?;
    }
    Brdb::new(&dst)?.write_pending("Optimize World", pending)?;

    println!("world written to {:?}", dst);

    Ok(())
}


