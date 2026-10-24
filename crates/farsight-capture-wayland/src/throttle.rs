//! Frame-rate limiting.

use std::time::{Duration, Instant};

/// The most frames per second we hand to the server.
pub const MAX_FPS: u32 = 30;

/// Admits at most one frame per interval.
#[derive(Debug, Clone)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    /// A throttle admitting at most `fps` frames per second (at least one).
    #[must_use]
    pub fn new(fps: u32) -> Self {
        Self {
            interval: Self::interval(fps),
            last: None,
        }
    }

    /// The time between frames at `fps` frames per second (at least one).
    #[must_use]
    pub fn interval(fps: u32) -> Duration {
        Duration::from_secs(1) / fps.max(1)
    }

    /// Whether a frame arriving at `now` may be delivered; if so, starts a new interval.
    pub fn admit(&mut self, now: Instant) -> bool {
        let due = self
            .last
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval);
        if due {
            self.last = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_one_frame_per_interval() {
        let mut throttle = Throttle::new(30);
        let start = Instant::now();
        assert!(throttle.admit(start));
        assert!(!throttle.admit(start + Duration::from_millis(10)));
        assert!(!throttle.admit(start + Duration::from_millis(33)));
        assert!(throttle.admit(start + Duration::from_millis(34)));
        assert!(!throttle.admit(start + Duration::from_millis(40)));
    }

    #[test]
    fn caps_sixty_hz_input_at_thirty() {
        let mut throttle = Throttle::new(MAX_FPS);
        let start = Instant::now();
        let admitted = (0..120)
            .filter(|i| throttle.admit(start + Duration::from_micros(16_667 * i)))
            .count();
        assert!((59..=61).contains(&admitted), "{admitted}");
    }

    #[test]
    fn zero_fps_is_treated_as_one() {
        let mut throttle = Throttle::new(0);
        let start = Instant::now();
        assert!(throttle.admit(start));
        assert!(!throttle.admit(start + Duration::from_millis(999)));
        assert!(throttle.admit(start + Duration::from_secs(1)));
    }
}
