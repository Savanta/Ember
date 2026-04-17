/// Runtime DND rule set.
#[derive(Debug, Clone)]
pub struct DndRules {
    pub enabled:  bool,
    pub schedule: Option<crate::config::DndSchedule>,
}

impl DndRules {
    pub fn new(enabled: bool, schedule: Option<crate::config::DndSchedule>) -> Self {
        Self { enabled, schedule }
    }

    /// Returns true when the notification should be suppressed (not shown as toast).
    /// Critical notifications always bypass DND.
    pub fn suppresses_toast(&self, urgency: crate::store::models::Urgency) -> bool {
        use crate::store::models::Urgency;
        let active = self.enabled || self.is_schedule_active();
        active && urgency != Urgency::Critical
    }

    /// Check whether the schedule window is currently active.
    fn is_schedule_active(&self) -> bool {
        let Some(sched) = &self.schedule else { return false; };
        let hour = local_hour();
        let from = sched.from % 24;
        let to   = sched.to   % 24;
        if from <= to {
            // Normal window: e.g. 9..17
            hour >= from && hour < to
        } else {
            // Overnight window: e.g. 22..8  (22:00 – 08:00)
            hour >= from || hour < to
        }
    }
}

/// Return the current local hour (0-23) using POSIX `localtime_r`.
fn local_hour() -> u8 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    // SAFETY: secs is a valid time_t value; tm is written by localtime_r.
    unsafe {
        unsafe extern "C" {
            fn localtime_r(timep: *const i64, result: *mut libc_tm) -> *mut libc_tm;
        }
        #[repr(C)]
        struct libc_tm {
            tm_sec:   i32, tm_min:   i32, tm_hour: i32,
            tm_mday:  i32, tm_mon:   i32, tm_year: i32,
            tm_wday:  i32, tm_yday:  i32, tm_isdst: i32,
            #[cfg(target_os = "linux")]
            tm_gmtoff: i64,
            #[cfg(target_os = "linux")]
            tm_zone: *const u8,
        }
        let mut tm: libc_tm = std::mem::zeroed();
        localtime_r(&secs, &mut tm);
        tm.tm_hour.clamp(0, 23) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::models::Urgency;

    #[test]
    fn dnd_off_never_suppresses() {
        let rules = DndRules::new(false, None);
        assert!(!rules.suppresses_toast(Urgency::Low));
        assert!(!rules.suppresses_toast(Urgency::Normal));
        assert!(!rules.suppresses_toast(Urgency::Critical));
    }

    #[test]
    fn dnd_on_suppresses_low_and_normal() {
        let rules = DndRules::new(true, None);
        assert!(rules.suppresses_toast(Urgency::Low));
        assert!(rules.suppresses_toast(Urgency::Normal));
    }

    #[test]
    fn dnd_on_never_suppresses_critical() {
        let rules = DndRules::new(true, None);
        assert!(!rules.suppresses_toast(Urgency::Critical));
    }
}
