optimizes a brickadia world file by:

- freezing all wheels and spheres around the world
- forcing all lights' cast shadows setting to off and forcing radius and brightness down to reasonable limitse
- getting rid of excess revisions (makes a huge difference in file size)
    - WARNING: right now it gets rid of ALL revisions, so be sure to make a backup before using this!

## how to use
> [!CAUTION]
> right now, this tool depends on a fix upstream in the brdb library!
> it hasn't been published yet, so running this right now will corrupt your world.
>
> if you want to fix it, get a local copy of the brdb library and replace the write_u8 function in `src/schema/write.rs` with:
```
/// Write the smallest possible unsigned integer representation of `value` to the buffer.
pub fn write_u8(buf: &mut impl Write, value: u64) -> Result<(), BrdbSchemaError> {
    if value <= 127 {
        rmp::encode::write_pfix(buf, (value as i8).try_into().unwrap())?;
    } else if value > 256 - 32 && value <= u8::MAX as u64 {
        rmp::encode::write_nfix(buf, value as i8)?;
    } else {
        rmp::encode::write_i8(buf, value as i8)?;
    }
    Ok(())
}
```

to run the tool, run this:
```
git clone https://github.com/Rose22/brdb_optimize.git
cargo run ~/path/to/your/world.brdb
```

for safety it doesn't overwrite your world file by default, but creates a new file with .optimized in its name. you can copy that over your old world file if you're sure it's okay!

## future plans
- freeze entire vehicles
- freeze all entities that aren't attached to any type of joint (bearings/sliders)
- omegga plugin that auto-runs this every night (or whatever interval you set)

