//! A critically damped spring for the pill's shape (ported from the island spike). It is evaluated from
//! elapsed time, so uneven frames do not change the motion, and retargeting keeps the current velocity.

/// The spike measured every transition of its reference recording at 19-20 rad/s: no overshoot, ~90 % of
/// the way at 0.2 s, settled by ~0.3 s.
const OMEGA: f32 = 20.0;

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    target: f32,
    /// Offset from the target and velocity at the spring's start.
    offset: f32,
    velocity: f32,
}

impl Spring {
    pub fn at_rest(value: f32) -> Self {
        Spring {
            target: value,
            offset: 0.0,
            velocity: 0.0,
        }
    }

    /// Starts at `from` and moves to `target`.
    pub fn between(from: f32, target: f32) -> Self {
        Spring {
            target,
            offset: from - target,
            velocity: 0.0,
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Value and velocity `t` seconds after the spring's start.
    fn state(&self, t: f32) -> (f32, f32) {
        let c2 = self.velocity + OMEGA * self.offset;
        let decay = (-OMEGA * t).exp();
        let value = self.target + (self.offset + c2 * t) * decay;
        let velocity = (c2 - OMEGA * (self.offset + c2 * t)) * decay;
        (value, velocity)
    }

    pub fn value(&self, t: f32) -> f32 {
        self.state(t).0
    }

    /// A new spring that starts now (`t` seconds into this one) from the current value and velocity.
    pub fn retarget(&self, t: f32, target: f32) -> Self {
        let (value, velocity) = self.state(t);
        Spring {
            target,
            offset: value - target,
            velocity,
        }
    }

    /// Under a hundredth of a pixel from the target, and still.
    pub fn settled(&self, t: f32) -> bool {
        let (value, velocity) = self.state(t);
        (value - self.target).abs() < 0.01 && velocity.abs() < 0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_to_the_target_without_overshoot() {
        let spring = Spring::between(100.0, 500.0);
        let mut previous = spring.value(0.0);
        assert_eq!(previous, 100.0);
        for step in 1..=200 {
            let value = spring.value(step as f32 * 0.005);
            assert!(
                value >= previous && value <= 500.0,
                "overshot at step {step}"
            );
            previous = value;
        }
        assert!(spring.settled(1.0));
    }

    #[test]
    fn is_close_to_ninety_percent_at_a_fifth_of_a_second() {
        let spring = Spring::between(0.0, 100.0);
        let value = spring.value(0.2);
        assert!((85.0..95.0).contains(&value), "got {value}");
    }

    #[test]
    fn retarget_keeps_the_value_and_velocity() {
        let spring = Spring::between(0.0, 100.0);
        let (value, velocity) = spring.state(0.1);
        let next = spring.retarget(0.1, 40.0);
        let (next_value, next_velocity) = next.state(0.0);
        assert!((next_value - value).abs() < 1e-3);
        assert!((next_velocity - velocity).abs() < 1e-3);
        assert_eq!(next.target(), 40.0);
    }
}
