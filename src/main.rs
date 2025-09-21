use std::{
    env,
    process,
    path::PathBuf
};
use brdb::{
    AsBrdbValue, Brdb, EntityChunkSoA, IntoReader, pending::BrPendingFs, schema::BrdbValue,
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

    // ------------------
    // Freeze all wheels
    // ------------------
    let mut entity_chunk_files = vec![];
    for index in db.entity_chunk_index()? {
        let entities = db.entity_chunk(index)?;

        let mut soa = EntityChunkSoA::default();
        for mut e in entities.into_iter() {
            let ent_type = e.data.get_schema_struct().unwrap().0;
            if ent_type.starts_with("Entity_Wheel") {
                e.frozen = true;
                println!("freezing entity {} of type {ent_type}", e.id.unwrap());
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
    // Disable shadows
    // ------------------
    let mut grid_ids = vec![1];

    // Collect dynamic brick grid IDs
    for index in db.entity_chunk_index()? {
        for e in db.entity_chunk(index)? {
            if e.data
                .get_schema_struct()
                .is_some_and(|s| s.0.as_ref() == "Entity_DynamicBrickGrid")
            {
                if let Some(id) = e.id {
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

        for index in chunks {
            if index.num_components == 0 {
                continue;
            }

            let (mut soa, components) = db.component_chunk(*grid, *index)?;
            let mut num_chunk_modified = 0;

            for mut s in components {
                if s.prop("bCastShadows")
                    .is_ok_and(|v| v.as_brdb_bool().unwrap_or_default())
                {
                    /*
                    println!(
                        "grid {grid} chunk {} mutating component {}",
                        *index,
                        s.get_name()
                    );
                    */
                    s.set_prop("bCastShadows", BrdbValue::Bool(false))?;
                    num_grid_modified += 1;
                    num_chunk_modified += 1;
                }

                soa.unwritten_struct_data.push(Box::new(s));
            }

            if num_chunk_modified > 0 {
                chunk_files.push((
                    format!("{}.mps", *index),
                    BrPendingFs::File(Some(soa.to_bytes(&component_schema)?)),
                ));
            }
        }

        if num_grid_modified > 0 {
            println!(
                "grid {grid} had {num_grid_modified} shadow-casting components disabled"
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

    println!();
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

    println!("World optimized and written to {:?}", dst);

    Ok(())
}

