//! Memory pressure and tab discarding, modelled on Chromium.
//!
//! * `components/memory_pressure/system_memory_pressure_evaluator_win.cc`:
//!   sample free physical memory every 5 s. On machines with at least
//!   1536 MB, pressure is moderate at or below 1000 MB available and
//!   critical at or below 400 MB (500 / 200 MB on smaller machines).
//!   Mithu uses the same numbers on every OS and reads them through the
//!   `sysinfo` crate, which reports `MemAvailable` on Linux and the
//!   equivalent on Windows and macOS.
//! * `chrome/browser/performance_manager/policies/page_discarding_helper.cc`
//!   and `discard_eligibility_policy.cc`: under pressure, discard one
//!   background tab at a time, least recently used first. The focused tab,
//!   a tab that is already discarded, and a tab that never finished loading
//!   are not candidates. A discarded tab keeps its title and URL in the tab
//!   strip and reloads when it is selected again.
//!
//! The Mithu-specific rule: under moderate pressure a tab must have been in
//! the background for [`MODERATE_MIN_HIDDEN`]; under critical pressure any
//! background tab qualifies.
//!
//! `MITHU_MEMORY_PRESSURE=moderate|critical` forces a level, the same role
//! as Chromium's `chrome://discards` page, so the mechanism can be tested
//! and measured without exhausting the machine's RAM.

use std::time::{Duration, Instant};

pub const SAMPLING_PERIOD: Duration = Duration::from_secs(5);
pub const LARGE_MEMORY_THRESHOLD_MB: u64 = 1536;
pub const LARGE_MODERATE_MB: u64 = 1000;
pub const LARGE_CRITICAL_MB: u64 = 400;
pub const SMALL_MODERATE_MB: u64 = 500;
pub const SMALL_CRITICAL_MB: u64 = 200;
pub const MODERATE_MIN_HIDDEN: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pressure {
    None,
    Moderate,
    Critical,
}

/// Chromium's thresholds for a machine with `total_mb` of RAM.
pub fn evaluate(available_mb: u64, total_mb: u64) -> Pressure {
    let (moderate, critical) = if total_mb >= LARGE_MEMORY_THRESHOLD_MB {
        (LARGE_MODERATE_MB, LARGE_CRITICAL_MB)
    } else {
        (SMALL_MODERATE_MB, SMALL_CRITICAL_MB)
    };
    if available_mb <= critical {
        Pressure::Critical
    } else if available_mb <= moderate {
        Pressure::Moderate
    } else {
        Pressure::None
    }
}

fn forced() -> Option<Pressure> {
    match std::env::var("MITHU_MEMORY_PRESSURE").ok()?.as_str() {
        "moderate" => Some(Pressure::Moderate),
        "critical" => Some(Pressure::Critical),
        "none" => Some(Pressure::None),
        _ => None,
    }
}

/// Polls system memory every [`SAMPLING_PERIOD`].
pub struct Monitor {
    system: Option<sysinfo::System>,
    next: Instant,
    forced: Option<Pressure>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            system: sysinfo::System::new().ok(),
            next: Instant::now() + SAMPLING_PERIOD,
            forced: forced(),
        }
    }

    /// The pressure level when a sample is due, otherwise `None`.
    pub fn poll(&mut self, now: Instant) -> Option<Pressure> {
        if now < self.next {
            return None;
        }
        self.next = now + SAMPLING_PERIOD;
        if let Some(level) = self.forced {
            return Some(level);
        }
        let system = self.system.as_mut()?;
        system.refresh_memory();
        let mb = 1024 * 1024;
        Some(evaluate(system.available_memory() / mb, system.total_memory() / mb))
    }

    /// Time until the next sample.
    pub fn due_in(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }
}

/// What the discard policy needs to know about one tab.
#[derive(Clone, Copy, Debug)]
pub struct Candidate {
    pub active: bool,
    pub discarded: bool,
    /// The tab finished at least one load (Chromium: has a main frame).
    pub loaded: bool,
    /// When the tab was last the focused tab.
    pub last_active: Instant,
}

/// The tab to discard at `level`, least recently used first.
pub fn pick_discard(tabs: &[Candidate], level: Pressure, now: Instant) -> Option<usize> {
    if level == Pressure::None {
        return None;
    }
    tabs.iter()
        .enumerate()
        .filter(|(_, tab)| !tab.active && !tab.discarded && tab.loaded)
        .filter(|(_, tab)| {
            level == Pressure::Critical
                || now.saturating_duration_since(tab.last_active) >= MODERATE_MIN_HIDDEN
        })
        .min_by_key(|(_, tab)| tab.last_active)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_thresholds() {
        assert_eq!(evaluate(4000, 16000), Pressure::None);
        assert_eq!(evaluate(1000, 16000), Pressure::Moderate);
        assert_eq!(evaluate(401, 16000), Pressure::Moderate);
        assert_eq!(evaluate(400, 16000), Pressure::Critical);
        assert_eq!(evaluate(600, 1024), Pressure::None);
        assert_eq!(evaluate(500, 1024), Pressure::Moderate);
        assert_eq!(evaluate(200, 1024), Pressure::Critical);
    }

    fn tab(active: bool, discarded: bool, loaded: bool, age_s: u64, now: Instant) -> Candidate {
        Candidate {
            active,
            discarded,
            loaded,
            last_active: now - Duration::from_secs(age_s),
        }
    }

    #[test]
    fn picks_the_least_recently_used_background_tab() {
        let now = Instant::now() + Duration::from_secs(3600);
        let tabs = [
            tab(false, false, true, 120, now),
            tab(true, false, true, 0, now),
            tab(false, false, true, 900, now),
            tab(false, true, true, 2000, now),
            tab(false, false, false, 3000, now),
        ];
        assert_eq!(pick_discard(&tabs, Pressure::Moderate, now), Some(2));
        assert_eq!(pick_discard(&tabs, Pressure::None, now), None);
    }

    #[test]
    fn moderate_spares_recently_hidden_tabs_critical_does_not() {
        let now = Instant::now() + Duration::from_secs(3600);
        let tabs = [tab(true, false, true, 0, now), tab(false, false, true, 5, now)];
        assert_eq!(pick_discard(&tabs, Pressure::Moderate, now), None);
        assert_eq!(pick_discard(&tabs, Pressure::Critical, now), Some(1));
    }

    #[test]
    fn never_discards_the_focused_tab() {
        let now = Instant::now() + Duration::from_secs(3600);
        let tabs = [tab(true, false, true, 5000, now)];
        assert_eq!(pick_discard(&tabs, Pressure::Critical, now), None);
    }
}
