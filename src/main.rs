/*
 * takes a brdb world file and optimizes it by:
 * - freezing all wheels and spheres
 * - TODO: freezing all entities not attached to any kind of joint (bearing/slider)
 * - TODO: freezing all physics grids that contain an engine (so basically, a vehicle)
 * - disabling castshadows on all lights everywhere
 * - forcing radius and brightness of all lights down to a reasonable limit
 * - TODO: stripping revisions to only the last 600 (keeps filesize small)
 *     (600 revisions = roughly 2 days assuming 5 minute autosave interval)
 * - neutralize stray weight components on the main grid
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
    let mut corrupted: bool = false;

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

            // skip corrupt chunks
            let (mut soa, components) = match db.component_chunk(*grid, *chunk) {
                Ok(value) => value,
                Err(e) => {
                    println!("[grid:{grid}][{}] found corrupt chunk! corruption: {e}", *chunk);
                    // if a corrupt chunk was found, dont risk saving the database
                    corrupted = true;
                    continue
                }
            };

            for mut component in components {
                let component_name = String::from(component.get_name());
                let mut modified: bool = false;

                if *grid == 1 {
                    // main grid

                    if component_name == "BrickComponentData_WeightBrick" {
                        // neutralize weight components on the main grid
                        let mut weight_modified: bool = false;

                        let weight_size = component.prop_mut("MassSize")?;
                        if weight_size.prop("X")?.as_brdb_i32()? > 0 {
                            weight_size.set_prop("X", BrdbValue::I32(0));
                            weight_modified = true;
                        }
                        if weight_size.prop("Y")?.as_brdb_i32()? > 0 {
                            weight_size.set_prop("Y", BrdbValue::I32(0));
                            weight_modified = true;
                        }
                        if weight_size.prop("Z")?.as_brdb_i32()? > 0 {
                            weight_size.set_prop("Z", BrdbValue::I32(0));
                            weight_modified = true;
                        }

                        let weight = component.prop("Mass")?.as_brdb_f32()?;
                        if weight > 0.0 {
                            component.set_prop("Mass", BrdbValue::F32(0.0));
                            weight_modified = true;
                        }

                        if weight_modified {
                            println!("[grid:{grid}][{}] weight neutralized", *chunk);
                            modified = true;
                            num_components_modified += 1;
                        }
                    }
                    if component_name == "BrickComponentData_WheelEngine" {
                        // neutralize wheel engine weight components on the main grid
                        let weight = component.prop("CustomMass")?.as_brdb_f32()?;
                        if weight > 0.0 {
                            println!("[grid:{grid}][{}] engine weight: was {weight}, neutralized", *chunk);
                            component.set_prop("CustomMass", BrdbValue::F32(0.0));

                            modified = true;
                        }
                    }
                }

                /*
                if component.prop("bAnglesArePercentages").is_ok() {
                    component.set_prop("bAnglesArePercentages", BrdbValue::Bool(false));
                }
                */

                if
                    component_name == "BrickComponentData_PointLight"
                    ||
                    component_name == "BrickComponentData_SpotLight"
                {
                    // light component

                    // force light radius down to 500
                    let component_radius = component.prop("Radius")?.as_brdb_f32()?;
                    if component_radius > 5000.0 {
                        // for some reason the game stores radiuses as thousands..
                        println!("[grid:{grid}][{}] light: radius exceeds 500, forcing down..", *chunk);
                        component.set_prop("Radius", BrdbValue::F32(5000.0));

                        modified = true;
                    }
                    // force light brightness down to 500
                    let component_brightness = component.prop("Brightness")?.as_brdb_f32()?;
                    if component_brightness > 400.0 {
                        println!("[grid:{grid}][{}] light: brightness exceeds 400, forcing down..", *chunk);
                        component.set_prop("Brightness", BrdbValue::F32(400.0));

                        modified = true;
                    }

                    // force cast shadows to off
                    let component_cast_shadows = component.prop("bCastShadows")?.as_brdb_bool()?;
                    if component_cast_shadows {
                        println!("[grid:{grid}][{}] light: disabling cast shadows..", *chunk);
                        component.set_prop("bCastShadows", BrdbValue::Bool(false))?;

                        modified = true;
                    }

                }

                if modified {
                    num_grid_modified += 1;
                    num_chunk_modified += 1;
                    num_components_modified += 1;
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

    if corrupted {
        println!("[ERROR] corruptions found! please read back through the log to see what went wrong.");
        println!("for safety, the world file was not written.");
        process::exit(1);
    }

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


