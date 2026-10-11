//! Send windows ("Mon–Fri, 9:00–17:00 local time") and due-date maths.
//! Generic over the time zone so tests can pin one; the app uses `chrono::Local`.

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendWindow {
    /// Monday first.
    pub days: [bool; 7],
    /// Minutes after local midnight; `start < end`.
    pub start_min: u32,
    pub end_min: u32,
}

impl Default for SendWindow {
    fn default() -> Self {
        SendWindow {
            days: [true, true, true, true, true, false, false],
            start_min: 9 * 60,
            end_min: 17 * 60,
        }
    }
}

impl SendWindow {
    /// Stored as "1,2,3,4,5" (ISO weekday numbers, Monday = 1).
    pub fn from_db(days: &str, start_min: i64, end_min: i64) -> Self {
        let mut d = [false; 7];
        for part in days.split(',') {
            if let Ok(n) = part.trim().parse::<usize>() {
                if (1..=7).contains(&n) {
                    d[n - 1] = true;
                }
            }
        }
        SendWindow {
            days: d,
            start_min: start_min.clamp(0, 24 * 60) as u32,
            end_min: end_min.clamp(0, 24 * 60) as u32,
        }
    }

    pub fn days_to_db(&self) -> String {
        self.days
            .iter()
            .enumerate()
            .filter(|(_, on)| **on)
            .map(|(i, _)| (i + 1).to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.days.iter().any(|d| *d) {
            return Err("Pick at least one day to send on.".into());
        }
        if self.start_min >= self.end_min {
            return Err("The send window must end after it starts.".into());
        }
        Ok(())
    }

    /// Length of one day's window in minutes.
    pub fn minutes_per_day(&self) -> u32 {
        self.end_min.saturating_sub(self.start_min)
    }

    pub fn is_open<Tz: TimeZone>(&self, at: DateTime<Utc>, tz: &Tz) -> bool {
        self.next_open(at, tz) == Some(at)
    }

    /// `at` itself if the window is open then, otherwise the start of the next
    /// open period. `None` only if no day is enabled.
    pub fn next_open<Tz: TimeZone>(&self, at: DateTime<Utc>, tz: &Tz) -> Option<DateTime<Utc>> {
        if self.validate().is_err() {
            return None;
        }
        let local_date = at.with_timezone(tz).date_naive();
        // Start one day back so a window that crosses a DST change is still found.
        for offset in -1..=8 {
            let date = local_date + Duration::days(offset);
            let weekday = date.weekday().num_days_from_monday() as usize;
            if !self.days[weekday] {
                continue;
            }
            let start = local_instant(tz, date, self.start_min);
            let end = local_instant(tz, date, self.end_min);
            if at < start {
                return Some(start);
            }
            if at < end {
                return Some(at);
            }
        }
        None
    }
}

/// The UTC instant of `minutes` past midnight on `date` in `tz`. A local time
/// skipped by a DST change moves forward to the first valid minute.
fn local_instant<Tz: TimeZone>(tz: &Tz, date: NaiveDate, minutes: u32) -> DateTime<Utc> {
    let mut m = minutes.min(24 * 60);
    loop {
        let naive = if m >= 24 * 60 {
            (date + Duration::days(1)).and_time(NaiveTime::MIN)
        } else {
            date.and_time(NaiveTime::from_hms_opt(m / 60, m % 60, 0).unwrap())
        };
        match tz.from_local_datetime(&naive) {
            LocalResult::Single(t) => return t.with_timezone(&Utc),
            LocalResult::Ambiguous(a, _) => return a.with_timezone(&Utc),
            LocalResult::None => m += 1,
        }
    }
}

/// When a follow-up becomes due: `delay_days` after the previous send, then
/// pushed into the next send window.
pub fn follow_up_due<Tz: TimeZone>(
    previous_sent: DateTime<Utc>,
    delay_days: i64,
    window: &SendWindow,
    tz: &Tz,
) -> DateTime<Utc> {
    let raw = previous_sent + Duration::days(delay_days.max(0));
    window.next_open(raw, tz).unwrap_or(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::parse_ts;
    use chrono_tz::Europe::London;

    fn t(s: &str) -> DateTime<Utc> {
        parse_ts(s).unwrap()
    }

    #[test]
    fn open_inside_weekday_hours() {
        let w = SendWindow::default();
        // Tue 13 Oct 2026, 10:00 BST = 09:00 UTC.
        assert!(w.is_open(t("2026-10-13T09:00:00Z"), &London));
        // 08:59 BST is before the window.
        assert!(!w.is_open(t("2026-10-13T07:59:00Z"), &London));
        // 17:00 BST exactly is closed (end is exclusive).
        assert!(!w.is_open(t("2026-10-13T16:00:00Z"), &London));
    }

    #[test]
    fn next_open_skips_weekend() {
        let w = SendWindow::default();
        // Fri 16 Oct 2026 18:00 BST -> Mon 19 Oct 09:00 BST (08:00 UTC).
        assert_eq!(
            w.next_open(t("2026-10-16T17:00:00Z"), &London),
            Some(t("2026-10-19T08:00:00Z"))
        );
    }

    #[test]
    fn next_open_handles_dst_change() {
        let w = SendWindow::default();
        // Clocks go back on Sun 25 Oct 2026. Sat 24 Oct -> Mon 26 Oct 09:00 GMT.
        assert_eq!(
            w.next_open(t("2026-10-24T12:00:00Z"), &London),
            Some(t("2026-10-26T09:00:00Z"))
        );
    }

    #[test]
    fn follow_up_due_pushed_into_window() {
        let w = SendWindow::default();
        // Sent Thu 15 Oct 16:30 BST; +1 day = Fri 16:30 BST, still open.
        assert_eq!(
            follow_up_due(t("2026-10-15T15:30:00Z"), 1, &w, &London),
            t("2026-10-16T15:30:00Z")
        );
        // +2 days = Sat -> Monday 09:00 BST.
        assert_eq!(
            follow_up_due(t("2026-10-15T15:30:00Z"), 2, &w, &London),
            t("2026-10-19T08:00:00Z")
        );
    }

    #[test]
    fn db_round_trip_and_validation() {
        let w = SendWindow::from_db("1,3,7", 600, 660);
        assert_eq!(w.days, [true, false, true, false, false, false, true]);
        assert_eq!(w.days_to_db(), "1,3,7");
        assert!(SendWindow::from_db("", 600, 660).validate().is_err());
        assert!(SendWindow::from_db("1", 700, 660).validate().is_err());
        assert_eq!(SendWindow::from_db("", 600, 660).next_open(Utc::now(), &London), None);
    }

    #[test]
    fn window_until_midnight_works() {
        let w = SendWindow::from_db("1,2,3,4,5,6,7", 0, 24 * 60);
        assert!(w.is_open(t("2026-10-13T23:30:00Z"), &London));
    }
}
