//! Map entity classname to the md3 that represents it in the world (`g_items.c` world models).

/// World model path of an item/flag/indicator classname, if the stock content has one.
pub fn model_for_classname(classname: &str) -> Option<String> {
    let c = classname.to_ascii_lowercase();
    if let Some(weapon) = c.strip_prefix("weapon_") {
        let dir = weapon_dir(weapon)?;
        return Some(format!("models/weapons/{dir}/{dir}.md3"));
    }
    if let Some(weapon) = c.strip_prefix("ammo_") {
        weapon_dir(weapon.trim_end_matches("_weak"))?;
        return Some("models/items/ammo/ammobox/ammobox.md3".into());
    }
    if let Some(tier) = c.strip_prefix("item_armor_") {
        return matches!(tier, "ga" | "ya" | "ra" | "shard")
            .then(|| format!("models/items/armor/{tier}/{tier}.md3"));
    }
    if let Some(size) = c.strip_prefix("item_health_") {
        return matches!(size, "small" | "medium" | "large" | "mega" | "ultra")
            .then(|| format!("models/items/health/{size}/{size}_health.md3"));
    }
    match c.as_str() {
        "item_quad" => Some("models/powerups/instant/quad.md3".into()),
        "item_warshell" => Some("models/powerups/instant/warshell_belt.md3".into()),
        "team_ctf_alphaflag" | "team_ctf_betaflag" | "team_ctf_redflag" | "team_ctf_blueflag" => {
            Some("models/objects/flag/flag.md3".into())
        }
        "misc_capture_area_indicator" => Some("models/objects/capture_area/indicator.md3".into()),
        _ => None,
    }
}

fn weapon_dir(weapon: &str) -> Option<&'static str> {
    Some(match weapon {
        "rocketlauncher" => "rlauncher",
        "grenadelauncher" => "glauncher",
        "riotgun" => "riotgun",
        "plasmagun" => "plasmagun",
        "machinegun" => "machinegun",
        "lasergun" => "lasergun",
        "electrobolt" => "electrobolt",
        "gunblade" => "gunblade",
        "instagun" => "instagun",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_items() {
        let m = |c| model_for_classname(c).unwrap();
        assert_eq!(
            m("weapon_rocketlauncher"),
            "models/weapons/rlauncher/rlauncher.md3"
        );
        assert_eq!(m("ITEM_ARMOR_RA"), "models/items/armor/ra/ra.md3");
        assert_eq!(
            m("item_health_mega"),
            "models/items/health/mega/mega_health.md3"
        );
        assert_eq!(m("ammo_plasmagun"), "models/items/ammo/ammobox/ammobox.md3");
        assert!(model_for_classname("info_player_start").is_none());
        assert!(model_for_classname("item_health").is_none());
    }
}
