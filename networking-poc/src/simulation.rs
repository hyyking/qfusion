#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputFrame {
    pub move_x: i16,
    pub move_y: i16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AuthoritativeState {
    pub tick: u32,
    pub x: i32,
    pub y: i32,
}

pub fn step(state: &mut AuthoritativeState, input: InputFrame) {
    state.tick = state.tick.wrapping_add(1);
    state.x = state.x.saturating_add(i32::from(input.move_x));
    state.y = state.y.saturating_add(i32::from(input.move_y));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advances_one_authoritative_state_step_per_tick() {
        let mut state = AuthoritativeState::default();
        step(
            &mut state,
            InputFrame {
                move_x: 1000,
                move_y: -25,
            },
        );
        assert_eq!(
            state,
            AuthoritativeState {
                tick: 1,
                x: 1000,
                y: -25
            }
        );
    }

    #[test]
    fn saturates_position_instead_of_overflowing() {
        let mut state = AuthoritativeState {
            tick: u32::MAX,
            x: i32::MAX,
            y: i32::MIN,
        };
        step(
            &mut state,
            InputFrame {
                move_x: 1,
                move_y: -1,
            },
        );
        assert_eq!(state.tick, 0);
        assert_eq!(state.x, i32::MAX);
        assert_eq!(state.y, i32::MIN);
    }
}
