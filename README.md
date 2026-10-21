# Ammo Extended
Is a mod for the [Highfleet Modloader](https://github.com/logdot/Highfleet-Modloader) that allows customization and addition of the ammos in the game.
Changes are made through the ammo_extended.json configuration file.
**Extensive documentation created from Rust is provided in each release in the doc folder.**

## Changing behaviour

For both versions, three independent fields control ammo selection and behavior:

- `caliber`: ammo identifier matched by a weapon's `m_weapon_caliber` in `OL.seria`.
- `behavior`: selects vanilla projectile spawning code, including its spread and muzzle flash.
- `logic`: selects projectile behavior such as laser guidance or proximity fuzing.

Normally the game uses the ammo's caliber identifier to select spawning code.
Ammo Extended instead stores `behavior` in native padding at offset `0xCC` and patches the game to read it.

Use a vanilla spawning behavior for a new caliber.
Regular shell behaviors include `5`, `10`, `15`, `20`, `25`, `30`, and `40`.
Rocket behaviors include `51`, `52`, `54`, and `55`.

Behavior `60` spawns mines with fixed damage; `70` has an unknown purpose; `80` applies a short-range force pulse.
The shipped config also contains `90` for harpoon, `100` for KAZ, and `0` for special entries including aircraft bombs and K-13.

These values are not an exhaustive validation enum.
The loader accepts a `u32` without enforcing a behavior whitelist.

## Shrapnel count, damage and fuse time

Each ammo entry accepts three independent optional overrides for shrapnel projectiles (`logic: 150`):

```json
{
  "caliber": 999,
  "behavior": 25,
  "logic": 150,
  "shrapnel_count": 20,
  "shrapnel_damage": 3.5,
  "shrapnel_fuse_time": 0.75
}
```

This is a partial entry; keep the other required ammo fields.
The overrides are stored per ammo entry, independently of caliber identifiers.
They only affect shrapnel runtime logic; other logic values do not use them.

- `shrapnel_count` sets the number of fragments per burst.
  Omit it to keep the vanilla count of **10**.
  It accepts an unsigned 32-bit integer (`0` through `4294967295`) without a custom range check or clamping.
  Larger counts are expensive.
- `shrapnel_damage` sets each fragment's **`explosive_power`**, not its final HP damage.
  Omit it to keep the vanilla value of **2**.
- `shrapnel_fuse_time` overrides the parent shrapnel shell's fuse in **seconds**.
  Omit it to keep the vanilla calculation: **100 world units divided by initial speed**.
  State updates are quantized.
  Therefore the actual burst occurs on a state update rather than necessarily at the exact configured instant.

**Warning: `shrapnel_count: 0` does NOT disable fragments.**
The original do-while loop underflows its 64-bit counter and almost certainly freezes or crashes the game when the shell bursts.
The mod logs a warning at config activation but accepts zero unchanged; it does not skip or clamp it.

## Missile lifetime

Ammo using missile spawning behaviour honors its configured lifetime in seconds instead of the vanilla hardcoded 10-second lifetime.

- **Steam 1.163:** uses the native ammo `lifespan` field.
Set `"lifespan": 25.0` for a 25-second missile lifetime.
- **Steam 1.151:** accepts an optional `lifespan` field, replacing the old config name `ttl`.
Omitting it preserves the vanilla 10-second missile lifetime.
The mod stores this value in `padding_164h`, so that padding field is now reserved by the mod.
Other 1.151 projectile lifetimes are unchanged.

For 1.163 shells with positive `penetrative_power`, the game forces lifespan to one second unless QoL's `enable_unblocked_ttl` patch is enabled.

## Plane payloads

`fire_delay` controls plane payload release timing; larger values mean slower release.
Standard guns are unaffected.
`salvo_size` is the number of payload releases per plane strafe.
aFter releasing this many times, the plane waits until it can break its strafe, gains distance, and approaches again.

`fire_delay` is separate from the aircraft gun reload time.
The 37mm aircraft gun uses delay `0.1` and salvo size `10` in 1.151, versus delay `0.05` and salvo size `20` in 1.163.
The 57mm aircraft gun has salvo size `7` in both versions.

`aircraft_strength_contribution` contributes to map-aircraft weapon-strength scoring, not projectile damage.
The game scales it by carried quantity for some weapon categories; gun contributions can be unscaled.

## Other fields

Other field meanings are extensively documented on `highfleet::v1_151::Ammo` and `highfleet::v1_163::Ammo`.
**You can find these in the docs folder of the release**.

## Steam 1.151 and 1.163 configuration: breaking schema change

Both versions now use the shared field names below.
Old configurations are rejected.
Missing required fields are errors.

| Old config key | New config key |
| --- | --- |
| `shell_kind` | `display_name` |
| `shell_kind2` | `display_name2` |
| `padding_cch` | `behavior` |
| `caliber` | `logic` |
| `index` | `caliber` |
| `ttl` | `lifespan` |
| `unknown_180h` (1.163) / `unknown_15ch` (1.151) | `salvo_size` |
| `unknown_150h` (1.151) | `shop_rarity` |
| `unknown_154h` (1.151) | `shop_ammount` |
| `unknown_158h` (1.151) | `fire_delay` |
| `unknown_160h` (1.151) | `aircraft_strength_contribution` |
| `padding_184h` (1.163, incorrectly classified as padding) | `aircraft_strength_contribution` |

Legacy keys `unknown_16ch`, `unknown_174h`, `unknown_178h`, and `unknown_17ch` are ignored.
Use `lifespan`, `shop_rarity`, `shop_ammount`, and `fire_delay`, respectively.
Other names, including `milimeterage` and `shop_ammount`, are unchanged.

**When migrating, move the old `caliber` value to `logic` before moving `index` to `caliber`.**
These fields have distinct meanings, not interchangeable values.
The shipped `ammo_extended/ammo_extended_1_151.json` and `ammo_extended/ammo_extended_1_163.json` are complete examples of the new schemas.

In old 1.163 configs, `padding_184h` contains the **IEEE-754 bits of a float**, not an integer strength value.
Migration must reinterpret those bits: for example, `1065353216` becomes `1.0`, not `1065353216.0`.
The shipped example has been converted without changing the underlying values.

1.151 has no `shell_enemy` field or native lifespan.
Its new config accepts optional `lifespan` for missile spawning only, defaulting to 10 seconds.

## Limitations
While you can add variations (e.g. piercing) of ammos, they are not infinite and cannot be purchased at shops.
This makes their utility very limited in campaigns.
