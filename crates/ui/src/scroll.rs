//! Animated wheel scrolling, modelled on Chromium's
//! `cc/animation/scroll_offset_animation_curve.cc`.
//!
//! A mouse-wheel notch does not jump the page. Chromium animates it with an
//! ease-in-out cubic Bézier (0.42, 0, 0.58, 1) whose duration shrinks as the
//! distance grows (`DurationBehavior::kInverseDelta`): 12 frames at 60 Hz for
//! 120 px or less, 6 frames for 480 px or more, linear in between. A notch
//! that arrives while the page is still moving retargets the running
//! animation; Chromium keeps the current velocity, and here the new segment
//! starts with an ease-out curve so the page does not stop for a frame.
//!
//! Precise touchpad deltas are already smooth and are not animated, the same
//! as Chromium.

use std::time::{Duration, Instant};

const DURATION_DIVISOR: f64 = 60.0;
const INVERSE_DELTA_RAMP_START_PX: f64 = 120.0;
const INVERSE_DELTA_RAMP_END_PX: f64 = 480.0;
const INVERSE_DELTA_MIN_FRAMES: f64 = 6.0;
const INVERSE_DELTA_MAX_FRAMES: f64 = 12.0;

/// Chromium's `kInverseDelta` segment duration for a scroll of `delta` px.
pub fn segment_duration(delta: f32) -> Duration {
    let slope = (INVERSE_DELTA_MIN_FRAMES - INVERSE_DELTA_MAX_FRAMES)
        / (INVERSE_DELTA_RAMP_END_PX - INVERSE_DELTA_RAMP_START_PX);
    let offset = INVERSE_DELTA_MAX_FRAMES - INVERSE_DELTA_RAMP_START_PX * slope;
    let frames = (offset + f64::from(delta.abs()) * slope)
        .clamp(INVERSE_DELTA_MIN_FRAMES, INVERSE_DELTA_MAX_FRAMES);
    Duration::from_secs_f64(frames / DURATION_DIVISOR)
}

/// `cubic-bezier(x1, y1, x2, y2)` evaluated at time fraction `t`.
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let curve = |a: f64, b: f64, s: f64| {
        let inv = 1.0 - s;
        3.0 * inv * inv * s * a + 3.0 * inv * s * s * b + s * s * s
    };
    // Solve x(s) = t by bisection; x is monotonic for 0 <= x1, x2 <= 1.
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.0)
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    from: f32,
    started: Instant,
    duration: Duration,
    /// Retargeted segments start at speed (ease-out) instead of from rest.
    ease_out: bool,
}

/// Turns wheel notches into per-frame scroll deltas.
#[derive(Clone, Debug, Default)]
pub struct ScrollAnimator {
    target: f32,
    emitted: f32,
    segment: Option<Segment>,
}

impl ScrollAnimator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue `delta` px of scrolling (positive scrolls down).
    pub fn add(&mut self, delta: f32, now: Instant) {
        if delta == 0.0 {
            return;
        }
        let running = self.segment.is_some();
        self.target += delta;
        self.segment = Some(Segment {
            from: self.emitted,
            started: now,
            duration: segment_duration(self.target - self.emitted),
            ease_out: running,
        });
    }

    pub fn animating(&self) -> bool {
        self.segment.is_some()
    }

    /// When the next frame is due, if an animation is running.
    pub fn next_frame(&self, now: Instant, interval: Duration) -> Option<Duration> {
        self.segment.map(|segment| {
            let end = segment.started + segment.duration;
            interval.min(end.saturating_duration_since(now))
        })
    }

    /// The scroll delta to apply for the frame at `now`.
    pub fn tick(&mut self, now: Instant) -> f32 {
        let Some(segment) = self.segment else {
            return 0.0;
        };
        let elapsed = now.saturating_duration_since(segment.started);
        let progress = if segment.duration.is_zero() {
            1.0
        } else {
            elapsed.as_secs_f64() / segment.duration.as_secs_f64()
        };
        let value = if progress >= 1.0 {
            self.segment = None;
            self.target
        } else {
            let eased = if segment.ease_out {
                cubic_bezier(0.0, 0.0, 0.58, 1.0, progress)
            } else {
                cubic_bezier(0.42, 0.0, 0.58, 1.0, progress)
            };
            segment.from + (self.target - segment.from) * eased as f32
        };
        let delta = value - self.emitted;
        self.emitted = value;
        if self.segment.is_none() {
            self.target = 0.0;
            self.emitted = 0.0;
        }
        delta
    }

    /// Drop any queued scroll (navigation, tab switch).
    pub fn cancel(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_follows_inverse_delta() {
        assert_eq!(segment_duration(64.0), Duration::from_secs_f64(12.0 / 60.0));
        assert_eq!(segment_duration(120.0), Duration::from_secs_f64(12.0 / 60.0));
        assert_eq!(segment_duration(-600.0), Duration::from_secs_f64(6.0 / 60.0));
        let mid = segment_duration(300.0).as_secs_f64() * 60.0;
        assert!((mid - 9.0).abs() < 1e-9, "{mid}");
    }

    #[test]
    fn bezier_endpoints_and_symmetry() {
        assert!(cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.0).abs() < 1e-9);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 1.0) - 1.0).abs() < 1e-9);
        assert!((cubic_bezier(0.42, 0.0, 0.58, 1.0, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_notch_is_spread_over_frames_and_sums_exactly() {
        let start = Instant::now();
        let mut scroll = ScrollAnimator::new();
        scroll.add(64.0, start);
        let mut total = 0.0;
        let mut frames = 0;
        let mut last = 0.0;
        for frame in 1..=20 {
            let delta = scroll.tick(start + Duration::from_millis(frame * 16));
            assert!(delta >= 0.0);
            if delta > 0.0 {
                frames += 1;
                last = delta;
            }
            total += delta;
        }
        assert!((total - 64.0).abs() < 1e-3, "{total}");
        assert!(frames >= 10, "spread over {frames} frames");
        assert!(last < 64.0 / 4.0, "eases out, last step {last}");
        assert!(!scroll.animating());
    }

    #[test]
    fn retarget_keeps_moving_and_reaches_the_sum() {
        let start = Instant::now();
        let mut scroll = ScrollAnimator::new();
        scroll.add(64.0, start);
        let mut total = scroll.tick(start + Duration::from_millis(48));
        scroll.add(64.0, start + Duration::from_millis(50));
        let first = scroll.tick(start + Duration::from_millis(66));
        assert!(first > 0.0, "no stall after a retarget");
        total += first;
        for frame in 5..40 {
            total += scroll.tick(start + Duration::from_millis(frame * 16));
        }
        assert!((total - 128.0).abs() < 1e-3, "{total}");
    }

    #[test]
    fn upward_scroll_is_negative() {
        let start = Instant::now();
        let mut scroll = ScrollAnimator::new();
        scroll.add(-64.0, start);
        let delta = scroll.tick(start + Duration::from_millis(100));
        assert!(delta < 0.0);
    }
}
