//! Time ranges, chart grains, refresh steps and resampling.

use chrono::{DateTime, Duration, DurationRound, Utc};
use serde::{Deserialize, Serialize};

use crate::azure::metrics::Interval;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TimeRange {
    #[serde(rename = "30m")]
    M30,
    #[serde(rename = "1h")]
    H1,
    #[default]
    #[serde(rename = "4h")]
    H4,
    #[serde(rename = "12h")]
    H12,
    #[serde(rename = "24h")]
    H24,
}

impl TimeRange {
    pub const ALL: [TimeRange; 5] = [
        TimeRange::M30,
        TimeRange::H1,
        TimeRange::H4,
        TimeRange::H12,
        TimeRange::H24,
    ];

    pub fn minutes(self) -> i64 {
        match self {
            TimeRange::M30 => 30,
            TimeRange::H1 => 60,
            TimeRange::H4 => 240,
            TimeRange::H12 => 720,
            TimeRange::H24 => 1440,
        }
    }

    pub fn grain(self) -> Interval {
        match self {
            TimeRange::M30 | TimeRange::H1 => Interval::PT1M,
            TimeRange::H4 | TimeRange::H12 => Interval::PT5M,
            TimeRange::H24 => Interval::PT15M,
        }
    }

    pub fn grain_minutes(self) -> i64 {
        match self.grain() {
            Interval::PT1M => 1,
            Interval::PT5M => 5,
            Interval::PT15M => 15,
            Interval::PT30M => 30,
            Interval::Full => self.minutes(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeRange::M30 => "30m",
            TimeRange::H1 => "1h",
            TimeRange::H4 => "4h",
            TimeRange::H12 => "12h",
            TimeRange::H24 => "24h",
        }
    }

    pub fn iso(self) -> &'static str {
        match self {
            TimeRange::M30 => "PT30M",
            TimeRange::H1 => "PT1H",
            TimeRange::H4 => "PT4H",
            TimeRange::H12 => "PT12H",
            TimeRange::H24 => "PT24H",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|r| *r == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.label() == s.trim())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// The query window for `range` ending at `now` truncated to the whole minute.
pub fn window(range: TimeRange, now: DateTime<Utc>) -> Window {
    let end = now.duration_trunc(Duration::minutes(1)).unwrap_or(now);
    Window {
        start: end - Duration::minutes(range.minutes()),
        end,
    }
}

pub const REFRESH_STEPS: [u64; 5] = [15, 30, 60, 120, 300];
pub const DEFAULT_REFRESH: u64 = 30;

/// The next longer (`up`) or shorter refresh step, clamped at both ends.
pub fn refresh_step(current: u64, up: bool) -> u64 {
    if up {
        REFRESH_STEPS
            .into_iter()
            .find(|s| *s > current)
            .unwrap_or(REFRESH_STEPS[REFRESH_STEPS.len() - 1])
    } else {
        REFRESH_STEPS
            .into_iter()
            .rev()
            .find(|s| *s < current)
            .unwrap_or(REFRESH_STEPS[0])
    }
}

/// Resamples to exactly `n` values, preserving peaks when downsampling.
/// Empty output buckets stay missing; upsampling uses nearest neighbours.
pub fn resample(values: &[Option<f64>], n: usize) -> Vec<Option<f64>> {
    if values.is_empty() || n == 0 {
        return vec![None; n];
    }
    if n >= values.len() {
        return (0..n).map(|i| values[i * values.len() / n]).collect();
    }
    (0..n)
        .map(|i| {
            let lo = i * values.len() / n;
            let hi = (i + 1) * values.len() / n;
            values[lo..hi].iter().flatten().copied().reduce(f64::max)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn grains_and_minutes() {
        assert_eq!(TimeRange::M30.grain(), Interval::PT1M);
        assert_eq!(TimeRange::H1.grain(), Interval::PT1M);
        assert_eq!(TimeRange::H4.grain(), Interval::PT5M);
        assert_eq!(TimeRange::H12.grain(), Interval::PT5M);
        assert_eq!(TimeRange::H24.grain(), Interval::PT15M);
        assert_eq!(TimeRange::H4.grain_minutes(), 5);
        assert_eq!(TimeRange::H24.grain_minutes(), 15);
        assert_eq!(TimeRange::M30.minutes(), 30);
        assert_eq!(TimeRange::H12.minutes(), 720);
    }

    #[test]
    fn labels_iso_next_parse() {
        assert_eq!(TimeRange::H4.label(), "4h");
        assert_eq!(TimeRange::M30.iso(), "PT30M");
        assert_eq!(TimeRange::H24.iso(), "PT24H");
        assert_eq!(TimeRange::H24.next(), TimeRange::M30);
        assert_eq!(TimeRange::M30.next(), TimeRange::H1);
        assert_eq!(TimeRange::parse("12h"), Some(TimeRange::H12));
        assert_eq!(TimeRange::parse("2h"), None);
        assert_eq!(TimeRange::default(), TimeRange::H4);
        assert_eq!(TimeRange::ALL.len(), 5);
    }

    #[test]
    fn serde_uses_labels() {
        assert_eq!(serde_json::to_string(&TimeRange::H12).unwrap(), "\"12h\"");
        assert_eq!(
            serde_json::from_str::<TimeRange>("\"30m\"").unwrap(),
            TimeRange::M30
        );
    }

    #[test]
    fn window_truncates_to_minute() {
        let now = Utc.with_ymd_and_hms(2026, 11, 4, 14, 3, 27).unwrap();
        let w = window(TimeRange::H1, now);
        assert_eq!(w.end, Utc.with_ymd_and_hms(2026, 11, 4, 14, 3, 0).unwrap());
        assert_eq!(
            w.start,
            Utc.with_ymd_and_hms(2026, 11, 4, 13, 3, 0).unwrap()
        );
    }

    #[test]
    fn refresh_steps() {
        assert_eq!(refresh_step(30, true), 60);
        assert_eq!(refresh_step(300, true), 300);
        assert_eq!(refresh_step(15, false), 15);
        assert_eq!(refresh_step(45, true), 60);
        assert_eq!(refresh_step(45, false), 30);
        assert_eq!(refresh_step(60, false), 30);
        assert_eq!(DEFAULT_REFRESH, 30);
    }

    #[test]
    fn resampling() {
        assert_eq!(
            resample(&[Some(1.), Some(2.)], 4),
            vec![Some(1.), Some(1.), Some(2.), Some(2.)]
        );
        assert_eq!(
            resample(&[Some(1.), Some(2.), Some(3.), Some(4.)], 2),
            vec![Some(2.), Some(4.)]
        );
        assert_eq!(resample(&[], 3), vec![None, None, None]);
        assert_eq!(resample(&[Some(1.)], 0), Vec::<Option<f64>>::new());
    }

    #[test]
    fn downsampling_preserves_a_spike_in_every_bucket() {
        for (buckets, slots) in [(96, 76), (144, 102), (48, 10)] {
            for spike in 0..buckets {
                let mut values = vec![Some(0.0); buckets];
                values[spike] = Some(42.0);
                let sampled = resample(&values, slots);
                assert_eq!(sampled.len(), slots);
                assert!(
                    sampled.contains(&Some(42.0)),
                    "lost spike at {spike} when resampling {buckets} to {slots}"
                );
            }
        }
    }

    #[test]
    fn resampling_preserves_missing_buckets_and_zeroes() {
        let values = [None, None, Some(0.0), None, Some(3.0), Some(7.0), None];
        assert_eq!(resample(&values, 3), vec![None, Some(0.0), Some(7.0)]);
        assert_eq!(
            resample(&[None, Some(0.0)], 4),
            vec![None, None, Some(0.0), Some(0.0)]
        );
        assert_eq!(resample(&values, values.len()), values);
        assert_eq!(resample(&[], 0), Vec::<Option<f64>>::new());
    }
}
