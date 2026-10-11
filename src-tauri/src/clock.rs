use chrono::{DateTime, SecondsFormat, Utc};

/// Time source. Production uses the system clock; tests use [`FakeClock`] so
/// due dates, windows and limits can be checked without waiting.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
pub struct FakeClock(pub std::sync::Mutex<DateTime<Utc>>);

#[cfg(test)]
impl FakeClock {
    pub fn at(rfc3339: &str) -> Self {
        FakeClock(std::sync::Mutex::new(parse_ts(rfc3339).unwrap()))
    }
    pub fn set(&self, t: DateTime<Utc>) {
        *self.0.lock().unwrap() = t;
    }
    pub fn advance(&self, d: chrono::Duration) {
        let mut t = self.0.lock().unwrap();
        *t += d;
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Timestamps are stored as fixed-width RFC 3339 UTC strings so they sort
/// correctly as text in SQLite.
pub fn ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
