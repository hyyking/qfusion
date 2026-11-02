//! Client-side pickup visibility: which item models are taken or respawned.
//!
//! This is a presentation shadow of the pickup rules. It owns its own [`Player`], never
//! touches the deterministic simulation, and its clock is accumulated frame time, so it
//! cannot change movement or recordings.

use std::time::Duration;

use qf_engine::content::bsp::entities::Entity;
use qf_engine::game::pickups::{PickupEvent, Pickups};
use qf_engine::game::{Player, Team};

/// Standing eye height above the player origin (`PLAYER_STAND_VIEWHEIGHT`).
const EYE_HEIGHT: f32 = 30.0;

pub struct PickupView {
    pickups: Pickups,
    player: Player,
    now_ms: f64,
}

impl PickupView {
    pub fn new(lump: &[Entity]) -> Self {
        let (pickups, _) = Pickups::from_entities(lump);
        let mut player = Player::new(Team::Alpha);
        player.alive = true;
        player.give_inventory();
        Self {
            pickups,
            player,
            now_ms: 0.0,
        }
    }

    /// Advances the clock and touches items at the viewer's position. Returns
    /// `(entity, visible)` for every model whose visibility changed.
    pub fn update(&mut self, eye: [f32; 3], elapsed: Duration) -> Vec<(usize, bool)> {
        self.now_ms += elapsed.as_secs_f64() * 1000.0;
        let now = self.now_ms as u64;
        self.player.origin = [eye[0], eye[1], eye[2] - EYE_HEIGHT];
        let mut events = self.pickups.update(now);
        events.extend(self.pickups.touch(now, 0, &mut self.player));
        events
            .into_iter()
            .filter_map(|event| {
                let (pickup, visible) = match event {
                    PickupEvent::PickedUp { pickup, .. } => (pickup, false),
                    PickupEvent::Respawned { pickup, .. } => (pickup, true),
                };
                Some((self.pickups.get(pickup)?.entity, visible))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lump(text: &str) -> Vec<Entity> {
        qf_engine::content::bsp::entities::parse_entities(text).unwrap()
    }

    #[test]
    fn taking_an_item_hides_it_until_respawn() {
        let mut view = PickupView::new(&lump(
            "{ \"classname\" \"item_health_small\" \"origin\" \"0 0 0\" }",
        ));
        let eye = [0.0, 0.0, EYE_HEIGHT];
        assert_eq!(view.update(eye, Duration::ZERO), vec![(0, false)]);
        assert!(view.update(eye, Duration::from_millis(100)).is_empty());
        assert_eq!(
            view.update([500.0, 0.0, 0.0], Duration::from_secs(60)),
            vec![(0, true)]
        );
    }
}
