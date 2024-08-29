# Ammo Extended
Is a mod for the [Highfleet Modloader](https://github.com/logdot/Highfleet-Modloader) that allows customization and addition of the ammos in the game.
Changes are made through the ammo_extended.json configuration file.
Extensive documentation created from Rust is provided in each distribution in the doc folder.

## Changing behaviour
Ammos are split into values and behaviours.
Normally the index (caliber) of an ammo has it's respective behaviour.

Ammo extended exposes the padding_cch field of each ammo to dictate the behaviour instead of the index.
This way you can add a new caliber and ammo to the game and use an existing behaviour for it.

## Limitations
While you can add variations (e.g. piercing) of ammos, they are not infinite and cannot be purchased at shops.
This makes their utility very limited in campaigns.
