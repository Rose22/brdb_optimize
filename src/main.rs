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
    /*
     * essential to know in understanding this program,
     * is that it takes a brdb world file and doesn't just modify the existing one,
     * but creates a brand new copy,
     * which involves copying every single thing over into the new file
     * while modifying anything that we want to change
     */

    // get cmdline arguments
    let args: Vec<String> = env::args().skip(1).take(1).collect();
    
    if args.is_empty() {
        println!("You must run the program with an argument that points to a world file.");
        process::exit(1);
    }

    // set up paths
    let src = PathBuf::from(&args[0]);
    let stem = src.file_stem().unwrap().to_string_lossy();
    let mut dst = src.with_file_name(format!("{stem}.optimized.brdb"));

    assert!(src.exists());

    // read brdb database and initialize variables
    println!("Reading file {:?}", args[0]);
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

    // loop through all entity chunks
    let mut entity_chunk_files = vec![];
    for chunk in db.entity_chunk_index()? {
        let entities = db.entity_chunk(chunk)?;

        /*
         * create a new entity chunk SoA (StructureOfArrays),
         * that we store our new entities in.
         *
         * SoA is defined in zeblote's msgpack-schema format:
         * https://gist.github.com/Zeblote/0fc682b9df1a3e82942b613ab70d8a04
         *
         * it's the way brdb files store this information
         */
        let mut soa = EntityChunkSoA::default();
        for mut entity in entities.into_iter() {
            // get the type of the entity as a string (basically its name)
            let ent_type = entity.data.get_schema_struct().unwrap().0;

            // if it's a wheel or a ball/sphere,
            if ent_type.starts_with("Entity_Wheel") || ent_type.starts_with("Entity_Ball") {
                // if this entity isn't frozen yet
                if !entity.frozen {
                    // then freeze it
                    println!("[entity:{}] freezing {ent_type}..", entity.id.unwrap());
                    entity.frozen = true;
                    num_entities_modified += 1;
                }
            } else {
                /*
                // unfreeze all entities
                println!("[entity:{}] unfreezing {ent_type}", e.id.unwrap());
                e.frozen = false;
                */
            }

            // add a new entity to our SoA
            soa.add_entity(&global_data, &entity, entity.id.unwrap() as u32);
        }

        // convert our entity SoA into a brdb .mps file that will be written to the brdb later
        // this contains the values for the properties of all the entities
        entity_chunk_files.push((
            format!("{chunk}.mps"),
            BrPendingFs::File(Some(soa.to_bytes(&entity_schema)?)),
        ));
    }

    /*
     * write all the entity chunk files we created
     * into the brdb file, as a new revision (patch)
     */
    let entities_patch = BrPendingFs::Root(vec![(
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

    // Collect all brick grid ID's (main grid + all dynamic/physics grids)
    let mut grid_ids = vec![1]; // we start out with grid id 1 (main grid) already inside
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

    /*
     * this will contain a modified copy
     * of all brick grids
     */
    let mut brick_grids_folder = vec![];

    // loop through all grids
    for grid in &grid_ids {
        // get all chunks in the grid
        let chunks = db.brick_chunk_index(*grid)?;
        let mut chunk_files = vec![];
        let mut num_grid_modified = 0;

        // loop through all chunks in this grid
        for chunk in chunks {
            // skip if there are no components
            if chunk.num_components == 0 {
                continue;
            }

            // get component data: the SoA (StructureOfArrays) and the actual components
            let (mut soa, components) = match db.component_chunk(*grid, *chunk) {
                Ok(value) => value,
                Err(e) => {
                    // skip corrupt chunks
                    
                    println!("[grid:{grid}][{}] found corrupt chunk! corruption: {e}", *chunk);
                    // if a corrupt chunk was found, dont risk saving the database
                    corrupted = true;
                    continue
                }
            };

            let mut num_chunk_modified = 0;
            // loop through components in this chunk
            for mut component in components {
                let component_name = String::from(component.get_name());
                let mut modified: bool = false;

                if *grid == 1 {
                    /*
                     * main grid (grid 1)
                     * this is the root grid, anything that's not a physics grid or entity
                     */

                    // if it's a weight component/brick
                    if component_name == "BrickComponentData_WeightBrick" {
                        let mut weight_modified: bool = false;

                        // set the mass size to (X:0,Y:0,Z:0)
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
                        // if mass is above 0,
                        if weight > 0.0 {
                            // set it to 0
                            component.set_prop("Mass", BrdbValue::F32(0.0));
                            weight_modified = true;
                        }

                        if weight_modified {
                            println!("[grid:{grid}][{}] weight neutralized", *chunk);
                            modified = true;
                            num_components_modified += 1;
                        }
                    }
                    // if it's a wheel engine component/brick
                    if component_name == "BrickComponentData_WheelEngine" {
                        let weight = component.prop("CustomMass")?.as_brdb_f32()?;

                        // if weight is above 0,
                        if weight > 0.0 {
                            // neutralize the weight (set it to 0)
                            println!("[grid:{grid}][{}] wheel engine weight neutralized", *chunk);
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

                // if it's any type of light,
                if
                    component_name == "BrickComponentData_PointLight"
                    ||
                    component_name == "BrickComponentData_SpotLight"
                {
                    // limit light radius to 500 or below
                    let component_radius = component.prop("Radius")?.as_brdb_f32()?;
                    if component_radius > 5000.0 {
                        println!("[grid:{grid}][{}] light: radius exceeds 500, forcing down..", *chunk);

                        // for some reason the game stores radiuses as thousands..
                        component.set_prop("Radius", BrdbValue::F32(5000.0));

                        modified = true;
                    }
                    // limit light brightness to 400 or below
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

                /*
                 * add the component to the current chunk's component StructureOfArrays
                 * IMPORTANT: regardless of if we modified it!
                 * because we're copying ALL components into the new file
                 */
                soa.unwritten_struct_data.push(Box::new(component));
            }

            if num_chunk_modified > 0 {
                /*
                 * now take the new chunk's SoA
                 * and convert it to an .mps file
                 * and add it to the vector array of files
                 * that we will write to the correct folder later
                 *
                 * example vector array:
                 *  - -1_-1_-1.mps
                 *  - 0_0_0.mps
                 * eventually becomes, in the filesystem:
                 *  - /World/0/Bricks/Grids/1/Components/-1_-1_-1.mps
                 *  - /World/0/Bricks/Grids/1/Components/0_0_0.mps
                 */
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

            /* 
             * now create a folder for the loop's current brick grid,
             * such as /World/0/Bricks/Grids/1/
             * then create a folder called Components inside it,
             * and insert all the chunk mps files we created earlier.
             * example:
             *  - /World/0/Bricks/Grids/
             *      - 1/ (this is the level we're currently working with)
             *          - Components/
             *              - -1_-1_-1.mps
             *              - 0_0_0.mps
             */
            brick_grids_folder.push((
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

    /*
     * create a revision (patch) out of all the
     * component data we gathered earlier
     */
    let components_patch = BrPendingFs::Root(vec![(
        "World".to_owned(),
        BrPendingFs::Folder(Some(vec![(
            "0".to_string(),
            BrPendingFs::Folder(Some(vec![(
                "Bricks".to_string(),
                BrPendingFs::Folder(Some(vec![(
                    "Grids".to_string(),
                    BrPendingFs::Folder(Some(brick_grids_folder)),
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
    // Write combined patch as a new revision
    // ------------------
    let pending = db
        .to_pending()?
        .with_patch(entities_patch)?
        .with_patch(components_patch)?;

    if dst.exists() {
        std::fs::remove_file(&dst)?;
    }
    Brdb::new(&dst)?.write_pending("Optimize World", pending)?;

    println!("world written to {:?}", dst);

    Ok(())
}


