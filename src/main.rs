/*
 * Opens a world and:
 * - freezes all wheels
 * - forces cast shadows off on all lights
 * - more coming
 */
use brdb::{Brdb, EntityChunkSoA, AsBrdbValue, schema::BrdbValue, IntoReader, pending::BrPendingFs};
use std::env;
use std::path::PathBuf;
use std::process::exit;

/// Opens a world and optimizes it
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).take(1).collect();
    
    if args.is_empty() {
        println!("You must run the program with an argument that points to a world file.");
        exit(1);
    }

    println!("Reading file {:?}", args[0]);
    let src = PathBuf::from(&args[0]);
    let dst = PathBuf::from(format!("{}.optimized.brdb", &args[0]));

    assert!(src.exists());
    
    let db = Brdb::open(src)?.into_reader();

    /*
    let entity_chunks = db.entity_chunk_index()?;
    let brick_chunks = db.brick_chunk_index(1)?;
    */
    let global_data = db.global_data()?;
    let components_schema = db.components_schema()?;
    let entity_schema = db.entities_schema()?;

    /*
     * freeze all wheels
     */
    /*
    for index in entity_chunks {
        // Entity_chunk loads entities and their entity data
        let entities = db.entity_chunk(index)?;

        // Re-assemble the soa. using add_entity ensures the extra data is correctly handled
        let mut soa = EntityChunkSoA::default();
        for mut e in entities.into_iter() {
            if e.data.get_schema_struct().unwrap().0.starts_with("Entity_Wheel") {
                e.frozen = true;
            }
            soa.add_entity(&global_data, &e, e.id.unwrap() as u32);
        }

        chunk_files.push((
            format!("{index}.mps"),
            // EntityChunkSoA::to_bytes ensures the extra data is written after the SoA data
            BrPendingFs::File(Some(soa.to_bytes(&entity_schema)?)),
        ));
    }

    // set all lights' cast shadow property to false
    let brick_chunks = db.brick_chunk_index(1)?;
    let components_schema = db.components_schema()?;
    for chunk in brick_chunks {
        if chunk.num_components > 0 {
            let (_soa, components) = db.component_chunk_soa(1, chunk.index)?;
            for c in components {
                let name = c.name.get_or(&components_schema, "Unknown Struct");
                match name {
                    "BrickComponentData_PointLight" | "BrickComponentData_SpotLight" => {
                        println!(
                            "Found light with cast shadows: {}",
                            c.prop("bCastShadows")?.as_brdb_bool()?
                        );
                    }
                    _ => {}
                }
            }
        }
    }
    */

    let mut grid_ids = vec![1];
    let mut entity_chunk_files = vec![];
    let mut component_chunk_files = vec![];
    let mut grids_files = vec![];
    let mut num_grid_modified = 0;

    // Iterate all entity chunks to find dynamic brick grids...
    for index in db.entity_chunk_index()? {
        for e in db.entity_chunk(index)? {
            // Ensure the chunk is a dynamic brick grid
            if !e
                .data
                .get_schema_struct()
                .is_some_and(|s| s.0.as_ref() == "Entity_DynamicBrickGrid")
            {
                continue;
            }
            let Some(id) = e.id else {
                continue;
            };
            grid_ids.push(id);
        }
    }

    // Iterate all grids (there can be bricks on entities)
    for grid in &grid_ids {
        let chunks = db.brick_chunk_index(*grid)?;

        // Iterate all chunks in the grid
        for chunk in chunks {
            let mut num_chunk_modified = 0;

            /*
             * optimize: freeze all wheels
             */
            // Entity_chunk loads entities and their entity data
            let entities = db.entity_chunk(chunk.index)?;

            // Re-assemble the soa
            let mut entity_soa = EntityChunkSoA::default();
            for mut e in entities.into_iter() {
                if e.data.get_schema_struct().unwrap().0.starts_with("Entity_Wheel") {
                    e.frozen = true;
                }
                entity_soa.add_entity(&global_data, &e, e.id.unwrap() as u32);
            }

            entity_chunk_files.push((
                format!("{}.mps", chunk.index),
                // EntityChunkSoA::to_bytes ensures the extra data is written after the SoA data
                BrPendingFs::File(Some(entity_soa.to_bytes(&entity_schema)?)),
            ));

            /*
             * optimize: disable cast shadows
             */
            if chunk.num_components > 0 {
                // Iterate all the components in the chunk
                let (mut component_soa, components) = db.component_chunk(*grid, *chunk)?;
                for mut s in components {
                    // Disable the shadow casting property if it's present and true
                    if s.prop("bCastShadows")
                        .is_ok_and(|v| v.as_brdb_bool().unwrap_or_default())
                    {
                        println!(
                            "grid {grid} chunk {} mutating component {}",
                            *chunk,
                            s.get_name()
                        );
                        s.set_prop("bCastShadows", BrdbValue::Bool(false))?;
                        num_grid_modified += 1;
                        num_chunk_modified += 1;
                    }

                    component_soa.unwritten_struct_data.push(Box::new(s));
                }

                if num_chunk_modified == 0 {
                    continue;
                }

                component_chunk_files.push((
                    format!("{}.mps", *chunk),
                    // ComponentChunkSoA::to_bytes ensures the extra data is written after the SoA data
                    BrPendingFs::File(Some(component_soa.to_bytes(&components_schema)?)),
                ));
            } else {
                println!("grid {grid} chunk {} has no components, skipping cast shadows disable..", *chunk);
            }
        }

        if num_grid_modified == 0 {
            println!("grid {grid} has no shadow-casting components");
        } else {
            println!(
                "grid {grid} has {num_grid_modified} shadow-casting components in {} files",
                component_chunk_files.len()
            );
        }

        grids_files.push((
            grid.to_string(),
            BrPendingFs::Folder(Some(vec![(
                "Components".to_string(),
                BrPendingFs::Folder(Some(component_chunk_files)),
            )])),
        ))
    }

    let entity_patch = BrPendingFs::Root(vec![(
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

    let shadow_patch = BrPendingFs::Root(vec![(
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

    // Use .to_pending_patch() if you want to update the same world
    let pending = db.to_pending()?.with_patch(entity_patch)?.with_patch(shadow_patch)?;
    if dst.exists() {
        std::fs::remove_file(&dst)?;
    }
    Brdb::new(&dst)?.write_pending("Optimization", pending)?;

    // Ensure entities can be read
    let db = Brdb::open(dst)?.into_reader();
    let chunks = db.entity_chunk_index()?;
    for index in chunks {
        let _ = db.entity_chunk(index)?;
    }

    // Ensure all the components can be read
    for grid in grid_ids {
        let chunks = db.brick_chunk_index(grid)?;
        for index in chunks {
            if index.num_components == 0 {
                continue;
            }
            let (_soa, _components) = db.component_chunk(grid, *index)?;
        }
    }

    Ok(())
}

