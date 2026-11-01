//! Entity lump key parsing shared by spawning and lookups.

use crate::content::bsp::entities::Entity;
use crate::simulation::pmove::{Vec3, angle_vectors};

use super::Gametype;
use super::components::{Links, Placement};

pub(super) fn class_is(entity: &Entity, classname: &str) -> bool {
    entity
        .get("classname")
        .is_some_and(|value| value.eq_ignore_ascii_case(classname))
}

/// Entities the legacy game frees while spawning, so they can never be targeted.
pub(super) fn freed_at_spawn(entity: &Entity) -> bool {
    class_is(entity, "info_null") || class_is(entity, "info_camp")
}

/// `G_CanSpawnEntity`: `notfree`/`notduel`/`notteam` and the `gametype` /
/// `not_gametype` lists. Worldspawn always spawns.
pub(super) fn passes_gametype_filter(entity: &Entity, gametype: &Gametype) -> bool {
    if class_is(entity, "worldspawn") {
        return true;
    }
    let flag = |key| entity.get(key).is_some_and(|value| atoi(value) != 0);
    let matches = |list: &str| {
        list.split([',', ' '])
            .any(|name| !name.is_empty() && name.eq_ignore_ascii_case(&gametype.name))
    };
    let duel = gametype.team_based && gametype.max_players_in_team == 1;
    let team = gametype.team_based && gametype.max_players_in_team != 1;
    if (!gametype.team_based && flag("notfree"))
        || (duel && (flag("notduel") || flag("notfree")))
        || (team && flag("notteam"))
    {
        return false;
    }
    entity.get("gametype").is_none_or(matches) && !entity.get("not_gametype").is_some_and(matches)
}

/// `angles`, or the `angle` hack that only sets yaw.
pub(super) fn entity_angles(entity: &Entity) -> Vec3 {
    if let Some(angles) = entity.get("angles").and_then(parse_vec3) {
        return angles;
    }
    [0.0, entity.get("angle").map_or(0.0, atof), 0.0]
}

/// `G_SetMovedir`: yaw -1/-2 mean straight up/down.
pub(super) fn movedir(entity: &Entity) -> Vec3 {
    let angles = entity_angles(entity);
    if angles == [0.0, -1.0, 0.0] {
        [0.0, 0.0, 1.0]
    } else if angles == [0.0, -2.0, 0.0] {
        [0.0, 0.0, -1.0]
    } else {
        angle_vectors(angles).0
    }
}

/// Origin and angles as the legacy spawn functions leave them.
pub(super) fn placement_of(entity: &Entity) -> Placement {
    let mut origin = entity
        .get("origin")
        .and_then(parse_vec3)
        .unwrap_or([0.0; 3]);
    // info_teleport_destination is spawned 16 units lower than where it is placed.
    if class_is(entity, "info_teleport_destination") {
        origin[2] += 16.0;
    }
    Placement {
        origin,
        angles: entity_angles(entity),
    }
}

pub(super) fn links_of(entity: &Entity) -> Links {
    let text = |key| entity.get(key).map(str::to_owned);
    Links {
        targetname: text("targetname"),
        target: text("target"),
        killtarget: text("killtarget"),
        message: text("message"),
        delay_ms: seconds_to_ms(entity.get("delay").map_or(0.0, atof)),
    }
}

/// `ent->wait * 1000` truncated to whole milliseconds.
pub(super) fn seconds_to_ms(seconds: f32) -> i64 {
    (seconds * 1000.0) as i64
}

pub(super) fn parse_vec3(value: &str) -> Option<Vec3> {
    let values: Vec<f32> = value
        .split_ascii_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let vector: Vec3 = values.try_into().ok()?;
    vector.iter().all(|v| v.is_finite()).then_some(vector)
}

/// `atof`: parses the longest numeric prefix, 0 when there is none.
pub(super) fn atof(value: &str) -> f32 {
    let value = value.trim_start();
    (1..=value.len())
        .rev()
        .filter(|&end| value.is_char_boundary(end))
        .find_map(|end| value[..end].parse::<f32>().ok().filter(|v| v.is_finite()))
        .unwrap_or(0.0)
}

pub(super) fn atoi(value: &str) -> i32 {
    atof(value) as i32
}
