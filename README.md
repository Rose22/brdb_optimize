optimizes a brickadia world file by:

- freezing all wheels and spheres around the world
- forcing all lights' cast shadows setting to off and forcing radius and brightness down to reasonable limits
- zeroing out all weight components attached to the main grid (meaning, not in a physics grid), including wheel engines
- getting rid of excess revisions (makes a huge difference in file size)
    - WARNING: right now it gets rid of ALL revisions, so be sure to make a backup before using this!

## how to use
to run the tool, first ensure you have rust installed. 
then run this:
```
git clone https://github.com/Rose22/brdb_optimize.git
cd brdb_optimize
cargo run ~/path/to/your/world.brdb
```

for safety it doesn't overwrite your world file by default, but creates a new file with .optimized in its name. you can copy that over your old world file if you're sure it's okay!

## future plans
- freeze entire vehicles
- freeze all entities that aren't attached to any type of joint (bearings/sliders)
- omegga plugin that auto-runs this every night (or whatever interval you set)

