use crate::Ammo;

#[cfg(feature = "1_151")]
fn default_missile_lifespan() -> f32 {
    10.0
}

/// User-facing ammo schema. Native padding repurposed by the mod is not exposed.
/// The generic parameter allows serialization to borrow the native ammo.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(bound(
    serialize = "T: serde::Serialize",
    deserialize = "T: serde::de::DeserializeOwned"
))]
struct ConfigAmmo<T = Ammo> {
    /// Selects vanilla spawning code, including its spread and muzzle flash.
    /// Stored in native padding by this mod, independently of the caliber identifier.
    behavior: u32,
    #[cfg(feature = "1_151")]
    #[serde(default = "default_missile_lifespan")]
    lifespan: f32,
    #[serde(flatten, with = "native_ammo")]
    ammo: T,
}

// Keep mod-owned padding out of the config without changing the shared native schema.
mod native_ammo {
    use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    pub fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: DeserializeOwned,
    {
        let mut fields = Map::<String, Value>::deserialize(deserializer)?;
        fields.insert("padding_cch".into(), 0.into());
        #[cfg(feature = "1_151")]
        fields.insert("padding_164h".into(), 0.into());
        serde_json::from_value(Value::Object(fields)).map_err(serde::de::Error::custom)
    }

    pub fn serialize<S, T>(ammo: &T, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: Serialize,
    {
        // Serialize through text to preserve f32's short decimal representation;
        // to_value widens floats to f64 and exposes their rounding artifacts.
        let json = serde_json::to_string(ammo).map_err(serde::ser::Error::custom)?;
        let mut fields: Value = serde_json::from_str(&json).map_err(serde::ser::Error::custom)?;
        if let Value::Object(fields) = &mut fields {
            fields.remove("padding_cch");
            #[cfg(feature = "1_151")]
            fields.remove("padding_164h");
        }
        fields.serialize(serializer)
    }
}

impl From<ConfigAmmo> for Ammo {
    fn from(config: ConfigAmmo) -> Self {
        let mut ammo = config.ammo;
        ammo.padding_cch = config.behavior;
        #[cfg(feature = "1_151")]
        {
            ammo.padding_164h = config.lifespan.to_bits();
        }
        ammo
    }
}

impl<'a> From<&'a Ammo> for ConfigAmmo<&'a Ammo> {
    fn from(ammo: &'a Ammo) -> Self {
        Self {
            behavior: ammo.padding_cch,
            #[cfg(feature = "1_151")]
            lifespan: f32::from_bits(ammo.padding_164h),
            ammo,
        }
    }
}

pub fn parse(contents: &str) -> Result<Vec<Ammo>, serde_json::Error> {
    let ammos: Vec<ConfigAmmo> = serde_json::from_str(contents)?;
    Ok(ammos.into_iter().map(Ammo::from).collect())
}

pub fn serialize(ammos: &[Ammo]) -> Result<String, serde_json::Error> {
    let configs: Vec<ConfigAmmo<&Ammo>> = ammos.iter().map(ConfigAmmo::from).collect();
    serde_json::to_string_pretty(&configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "1_151")]
    const CONFIG: &str = include_str!("../ammo_extended_1_151.json");
    #[cfg(not(feature = "1_151"))]
    const CONFIG: &str = include_str!("../ammo_extended_1_163.json");

    #[test]
    fn storage_matches_patch_offsets_and_native_ammo_size() {
        assert_eq!(std::mem::offset_of!(Ammo, padding_cch), 0xcc);
        #[cfg(feature = "1_151")]
        {
            assert_eq!(std::mem::offset_of!(Ammo, logic), 0x130);
            assert_eq!(std::mem::offset_of!(Ammo, caliber), 0x134);
            assert_eq!(std::mem::offset_of!(Ammo, salvo_size), 0x15c);
            assert_eq!(
                std::mem::offset_of!(Ammo, aircraft_strength_contribution),
                0x160
            );
            assert_eq!(std::mem::offset_of!(Ammo, padding_164h), 0x164);
            assert_eq!(std::mem::size_of::<Ammo>(), 0x168);
        }
        #[cfg(not(feature = "1_151"))]
        {
            assert_eq!(std::mem::offset_of!(Ammo, logic), 0x150);
            assert_eq!(std::mem::offset_of!(Ammo, caliber), 0x154);
            assert_eq!(std::mem::offset_of!(Ammo, lifespan), 0x16c);
            assert_eq!(std::mem::offset_of!(Ammo, salvo_size), 0x180);
            assert_eq!(
                std::mem::offset_of!(Ammo, aircraft_strength_contribution),
                0x184
            );
            assert_eq!(std::mem::size_of::<Ammo>(), 0x188);
        }
    }

    #[test]
    fn missile_lifetime_is_configurable_without_changing_other_ammo_values() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        let missile = config
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|ammo| ammo["behavior"] == 51)
            .unwrap();
        missile["lifespan"] = 25.5.into();
        let ammos = parse(&config.to_string()).unwrap();
        let missile = ammos.iter().find(|ammo| ammo.padding_cch == 51).unwrap();
        #[cfg(feature = "1_151")]
        assert_eq!(f32::from_bits(missile.padding_164h), 25.5);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(missile.lifespan, 25.5);
        assert_eq!(missile.logic, 130);
        assert_eq!(missile.caliber, 51);
    }

    #[test]
    fn new_schema_round_trips_all_config_values() {
        let expected: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        let actual: serde_json::Value =
            serde_json::from_str(&serialize(&parse(CONFIG).unwrap()).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn behavior_logic_and_caliber_are_independent() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        config[0]["behavior"] = 25.into();
        config[0]["logic"] = 140.into();
        config[0]["caliber"] = 999.into();
        let ammos = parse(&config.to_string()).unwrap();
        assert_eq!(ammos[0].padding_cch, 25);
        assert_eq!(ammos[0].logic, 140);
        assert_eq!(ammos[0].caliber, 999);
    }

    #[test]
    fn aircraft_strength_is_stored_as_float_not_padding_bits() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        config[0]["aircraft_strength_contribution"] = 3.5.into();
        let ammos = parse(&config.to_string()).unwrap();
        assert_eq!(ammos[0].aircraft_strength_contribution, 3.5);
    }

    #[test]
    fn rejects_old_format() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for ammo in config.as_array_mut().unwrap() {
            let fields = ammo.as_object_mut().unwrap();
            for (new, old) in [
                ("caliber", "index"),
                ("logic", "caliber"),
                ("display_name", "shell_kind"),
                ("display_name2", "shell_kind2"),
                ("behavior", "padding_cch"),
            ] {
                let value = fields.remove(new).unwrap();
                fields.insert(old.into(), value);
            }
        }
        assert!(parse(&config.to_string()).is_err());
    }

    #[test]
    fn rejects_missing_required_fields() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for field in [
            "display_name",
            "display_name2",
            "behavior",
            "logic",
            "caliber",
            "salvo_size",
            "aircraft_strength_contribution",
            "shop_rarity",
            "shop_ammount",
            "fire_delay",
        ] {
            let mut config = original.clone();
            config[0].as_object_mut().unwrap().remove(field);
            assert!(
                parse(&config.to_string()).is_err(),
                "accepted missing {field}"
            );
        }
    }

    #[test]
    fn ignores_unknown_fields_without_overriding_mod_storage() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(&serialize(&parse(CONFIG).unwrap()).unwrap()).unwrap();
        for field in [
            "shell_kind",
            "shell_kind2",
            "padding_cch",
            "index",
            "ttl",
            "unknown_180h",
            "unknown_16ch",
            "unknown_174h",
            "unknown_178h",
            "unknown_17ch",
            "unknown_150h",
            "unknown_154h",
            "unknown_158h",
            "unknown_15ch",
            "unknown_160h",
            "padding_164h",
            "padding_184h",
            "behaviour",
        ] {
            let mut config = original.clone();
            config[0][field] = 0.into();
            assert!(
                serde_json::from_str::<serde_json::Value>(
                    &serialize(&parse(&config.to_string()).unwrap()).unwrap()
                )
                .unwrap()
                    == expected,
                "unknown {field} changed config values"
            );
        }
    }

    #[cfg(not(feature = "1_151"))]
    #[test]
    fn native_lifespan_is_required() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        config[0].as_object_mut().unwrap().remove("lifespan");
        assert!(parse(&config.to_string()).is_err());
    }

    #[cfg(feature = "1_151")]
    #[test]
    fn omitted_1_151_lifespan_keeps_ten_second_missile_lifetime() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for ammo in config.as_array_mut().unwrap() {
            ammo.as_object_mut().unwrap().remove("lifespan");
        }
        let ammos = parse(&config.to_string()).unwrap();
        assert!(ammos
            .iter()
            .all(|ammo| f32::from_bits(ammo.padding_164h) == 10.0));
    }

    #[cfg(feature = "1_151")]
    #[test]
    fn ignores_1_163_only_enemy_sound() {
        let mut config: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        config[0]["shell_enemy"] = "shell_out_enemy_med".into();
        let actual: serde_json::Value =
            serde_json::from_str(&serialize(&parse(&config.to_string()).unwrap()).unwrap())
                .unwrap();
        let expected: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        assert_eq!(actual, expected);
    }
}
