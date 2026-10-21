use crate::{shrapnel::Settings, Ammo};

/// Native ammo and mod-only settings in matching config-file order.
/// `shrapnel[i]` belongs to `ammos[i]`, independently of caliber or logic.
pub struct Config {
    pub ammos: Vec<Ammo>,
    pub shrapnel: Vec<Settings>,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shrapnel_damage: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shrapnel_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shrapnel_fuse_time: Option<f32>,
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
            shrapnel_damage: None,
            shrapnel_count: None,
            shrapnel_fuse_time: None,
            #[cfg(feature = "1_151")]
            lifespan: f32::from_bits(ammo.padding_164h),
            ammo,
        }
    }
}

pub fn parse(contents: &str) -> Result<Config, serde_json::Error> {
    let configs: Vec<ConfigAmmo> = serde_json::from_str(contents)?;
    let mut ammos = Vec::with_capacity(configs.len());
    let mut shrapnel = Vec::with_capacity(configs.len());
    for config in configs {
        shrapnel.push(Settings {
            damage: config.shrapnel_damage,
            count: config.shrapnel_count,
            fuse_time: config.shrapnel_fuse_time,
        });
        ammos.push(Ammo::from(config));
    }
    Ok(Config { ammos, shrapnel })
}

/// Serialize per-ammo overrides without dropping them or changing index alignment.
pub fn serialize_with_settings(
    ammos: &[Ammo],
    settings: &[Settings],
) -> Result<String, serde_json::Error> {
    if ammos.len() != settings.len() {
        return Err(serde::ser::Error::custom(
            "ammo and shrapnel settings lengths differ",
        ));
    }
    let configs: Vec<_> = ammos
        .iter()
        .zip(settings)
        .map(|(ammo, settings)| {
            let mut config = ConfigAmmo::from(ammo);
            config.shrapnel_damage = settings.damage;
            config.shrapnel_count = settings.count;
            config.shrapnel_fuse_time = settings.fuse_time;
            config
        })
        .collect();
    serde_json::to_string_pretty(&configs)
}

pub fn serialize(ammos: &[Ammo]) -> Result<String, serde_json::Error> {
    let configs: Vec<ConfigAmmo<&Ammo>> = ammos.iter().map(ConfigAmmo::from).collect();
    serde_json::to_string_pretty(&configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keep native-schema tests focused on the ammo half of the parsed config.
    fn parse(contents: &str) -> Result<Vec<Ammo>, serde_json::Error> {
        super::parse(contents).map(|config| config.ammos)
    }

    #[cfg(feature = "1_151")]
    const CONFIG: &str = include_str!("../ammo_extended_1_151.json");
    #[cfg(not(feature = "1_151"))]
    const CONFIG: &str = include_str!("../ammo_extended_1_163.json");

    #[test]
    fn omitted_shrapnel_settings_remain_vanilla_and_are_not_serialized() {
        let config = super::parse(CONFIG).unwrap();
        assert_eq!(config.shrapnel.len(), config.ammos.len());
        assert!(config
            .shrapnel
            .iter()
            .all(|settings| *settings == Settings::default()));
        let json = serialize_with_settings(&config.ammos, &config.shrapnel).unwrap();
        assert!(!json.contains("shrapnel_damage"));
        assert!(!json.contains("shrapnel_count"));
        assert!(!json.contains("shrapnel_fuse_time"));
        let expected: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap(),
            expected
        );
    }

    #[test]
    fn shrapnel_overrides_are_independent_and_round_trip_without_native_changes() {
        let original = parse(CONFIG).unwrap();
        let mut json: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        // Settings belong to the array index, even for duplicate identifiers and non-150 logic.
        json[0]["logic"] = 140.into();
        json[1]["caliber"] = json[0]["caliber"].clone();
        json[0]["shrapnel_damage"] = 3.5.into();
        json[1]["shrapnel_fuse_time"] = 0.75.into();
        json[2]["shrapnel_count"] = 20.into();
        json[2]["shrapnel_damage"] = 0.0.into();
        json[2]["shrapnel_fuse_time"] = 0.0.into();
        let config = super::parse(&json.to_string()).unwrap();
        assert_eq!(
            config.shrapnel[0],
            Settings {
                damage: Some(3.5),
                count: None,
                fuse_time: None
            }
        );
        assert_eq!(
            config.shrapnel[1],
            Settings {
                damage: None,
                count: None,
                fuse_time: Some(0.75)
            }
        );
        assert_eq!(
            config.shrapnel[2],
            Settings {
                damage: Some(0.0),
                count: Some(20),
                fuse_time: Some(0.0)
            }
        );
        assert!(config.shrapnel[3..]
            .iter()
            .all(|settings| *settings == Settings::default()));
        let serialized = serialize_with_settings(&config.ammos, &config.shrapnel).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized).unwrap(),
            json
        );
        let round_trip = super::parse(&serialized).unwrap();
        assert_eq!(round_trip.shrapnel, config.shrapnel);
        for (index, ammo) in config.ammos.iter().enumerate() {
            let mut expected = serde_json::to_value(&original[index]).unwrap();
            if index == 0 {
                expected["logic"] = 140.into();
            }
            if index == 1 {
                expected["caliber"] = json[0]["caliber"].clone();
            }
            assert_eq!(serde_json::to_value(ammo).unwrap(), expected);
        }
        let debug_dump = serialize(&config.ammos).unwrap();
        assert!(!debug_dump.contains("shrapnel_damage"));
        assert!(!debug_dump.contains("shrapnel_count"));
        assert!(!debug_dump.contains("shrapnel_fuse_time"));
    }

    #[test]
    fn count_only_settings_round_trip_unchanged_without_native_changes() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        let original_ammos = parse(CONFIG).unwrap();
        for count in [0, 1, 10, 20, u32::MAX] {
            let mut json = original.clone();
            json[0]["shrapnel_count"] = count.into();
            let config = super::parse(&json.to_string()).unwrap();
            assert_eq!(
                config.shrapnel[0],
                Settings {
                    count: Some(count),
                    ..Settings::default()
                }
            );
            assert!(config.shrapnel[1..]
                .iter()
                .all(|settings| *settings == Settings::default()));
            assert_eq!(
                serde_json::to_value(&config.ammos).unwrap(),
                serde_json::to_value(&original_ammos).unwrap()
            );
            let serialized = serialize_with_settings(&config.ammos, &config.shrapnel).unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&serialized).unwrap(),
                json
            );
            assert_eq!(super::parse(&serialized).unwrap().shrapnel, config.shrapnel);
            assert!(!serialize(&config.ammos).unwrap().contains("shrapnel_count"));
        }
    }

    #[test]
    fn shrapnel_count_uses_normal_u32_deserialization() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for value in [
            "-1",
            "4294967296",
            "1.5",
            "1.0",
            "\"10\"",
            "true",
            "[]",
            "{}",
        ] {
            let mut json = original.clone();
            json[0]["shrapnel_count"] = "REPLACE_VALUE".into();
            let text = json.to_string().replace("\"REPLACE_VALUE\"", value);
            assert!(
                super::parse(&text).is_err(),
                "accepted shrapnel_count={value}"
            );
        }
    }

    #[test]
    fn accepts_negative_shrapnel_values_and_f32_overflow() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for field in ["shrapnel_damage", "shrapnel_fuse_time"] {
            for (text, expected) in [
                ("-1", -1.0f32),
                ("-0.001", -0.001),
                ("1e39", f32::INFINITY),
                ("-1e39", f32::NEG_INFINITY),
            ] {
                let mut json = original.clone();
                json[0][field] = "REPLACE_VALUE".into();
                let text = json.to_string().replace("\"REPLACE_VALUE\"", text);
                let config = super::parse(&text).unwrap();
                let settings = config.shrapnel[0];
                let actual = if field == "shrapnel_damage" {
                    settings.damage
                } else {
                    settings.fuse_time
                };
                assert_eq!(actual, Some(expected));
            }
        }
    }

    #[test]
    fn rejects_non_numeric_shrapnel_values_and_json_number_overflow() {
        let original: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        for field in ["shrapnel_damage", "shrapnel_fuse_time"] {
            for value in ["1e400", "\"NaN\"", "\"Infinity\"", "true", "[]", "{}"] {
                let mut json = original.clone();
                json[0][field] = "REPLACE_VALUE".into();
                let text = json.to_string().replace("\"REPLACE_VALUE\"", value);
                assert!(super::parse(&text).is_err(), "accepted {field}={value}");
            }
        }
    }

    #[test]
    fn null_shrapnel_settings_are_omitted_like_defaults() {
        let mut json: serde_json::Value = serde_json::from_str(CONFIG).unwrap();
        json[0]["shrapnel_damage"] = serde_json::Value::Null;
        json[0]["shrapnel_count"] = serde_json::Value::Null;
        json[0]["shrapnel_fuse_time"] = serde_json::Value::Null;
        let config = super::parse(&json.to_string()).unwrap();
        assert_eq!(config.shrapnel[0], Settings::default());
        let serialized = serialize_with_settings(&config.ammos, &config.shrapnel).unwrap();
        assert!(!serialized.contains("shrapnel_count"));
    }

    #[test]
    fn serialization_rejects_misalignment_but_does_not_validate_float_values() {
        let mut config = super::parse(CONFIG).unwrap();
        assert!(serialize_with_settings(&config.ammos, &config.shrapnel[1..]).is_err());
        for value in [-1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            config.shrapnel[0] = Settings {
                damage: Some(value),
                count: None,
                fuse_time: Some(value),
            };
            let json = serialize_with_settings(&config.ammos, &config.shrapnel).unwrap();
            let json: serde_json::Value = serde_json::from_str(&json).unwrap();
            // serde_json represents non-finite floats as null, not JSON numbers.
            let expected = if value.is_finite() {
                serde_json::json!(value)
            } else {
                serde_json::Value::Null
            };
            assert_eq!(json[0]["shrapnel_damage"], expected);
            assert_eq!(json[0]["shrapnel_fuse_time"], expected);
        }
    }

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
