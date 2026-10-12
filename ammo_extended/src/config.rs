use crate::Ammo;

#[cfg(feature = "1_151")]
#[derive(serde::Deserialize)]
struct ConfigAmmo {
    #[serde(flatten)]
    ammo: Ammo,
    #[serde(default = "default_missile_ttl")]
    ttl: f32,
}

#[cfg(feature = "1_151")]
fn default_missile_ttl() -> f32 {
    10.0
}

pub fn parse(contents: &str) -> Result<Vec<Ammo>, serde_json::Error> {
    #[cfg(feature = "1_151")]
    {
        let ammos: Vec<ConfigAmmo> = serde_json::from_str(contents)?;
        Ok(ammos
            .into_iter()
            .map(|mut config| {
                config.ammo.padding_164h = config.ttl.to_bits();
                config.ammo
            })
            .collect())
    }
    #[cfg(not(feature = "1_151"))]
    serde_json::from_str(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_storage_matches_patch_offset_and_native_ammo_size() {
        #[cfg(feature = "1_151")]
        {
            assert_eq!(std::mem::offset_of!(Ammo, padding_164h), 0x164);
            assert_eq!(std::mem::size_of::<Ammo>(), 0x168);
        }
        #[cfg(not(feature = "1_151"))]
        {
            assert_eq!(std::mem::offset_of!(Ammo, ttl), 0x16c);
            assert_eq!(std::mem::size_of::<Ammo>(), 0x188);
        }
    }

    #[test]
    fn missile_ttl_is_configurable_without_changing_other_ammo_values() {
        #[cfg(feature = "1_151")]
        let contents = include_str!("../ammo_extended_1_151.json");
        #[cfg(not(feature = "1_151"))]
        let contents = include_str!("../ammo_extended_1_163.json");
        let mut config: serde_json::Value = serde_json::from_str(contents).unwrap();
        let missile = config
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|ammo| ammo["padding_cch"] == 51)
            .unwrap();
        missile["ttl"] = 25.5.into();
        let ammos = parse(&config.to_string()).unwrap();
        let missile = ammos.iter().find(|ammo| ammo.padding_cch == 51).unwrap();
        #[cfg(feature = "1_151")]
        assert_eq!(f32::from_bits(missile.padding_164h), 25.5);
        #[cfg(not(feature = "1_151"))]
        assert_eq!(missile.ttl, 25.5);
        assert_eq!(missile.caliber, 130);
    }

    #[cfg(feature = "1_151")]
    #[test]
    fn old_1_151_configs_keep_ten_second_missile_lifetime() {
        let mut config: serde_json::Value =
            serde_json::from_str(include_str!("../ammo_extended_1_151.json")).unwrap();
        for ammo in config.as_array_mut().unwrap() {
            ammo.as_object_mut().unwrap().remove("ttl");
        }
        let ammos = parse(&config.to_string()).unwrap();
        assert!(ammos
            .iter()
            .all(|ammo| f32::from_bits(ammo.padding_164h) == 10.0));
    }
}
