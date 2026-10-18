//! Funding as the keeper weighs it: Phoenix's hourly rates averaged over a day, a week and a
//! month, annualized. Positive funding is paid by longs to shorts: the vault's income.

const YEAR_SECONDS: f64 = 365.0 * 86_400.0;
const DAY: i64 = 86_400;

/// A market's funding, annualized, in percent (12.0 is 12% a year on the short's notional).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Funding {
    pub apr_24h: f64,
    pub apr_7d: f64,
    pub apr_30d: f64,
    /// When the latest rate was paid (unix seconds).
    pub as_of: i64,
}

impl Funding {
    /// What the keeper ranks markets by: the week and the month together, so a leg is neither
    /// chased on a spike nor held through a slow turn.
    pub fn score(&self) -> f64 {
        (self.apr_7d + self.apr_30d) / 2.0
    }

    /// The same rate in every window: for tests and simulations.
    pub fn flat(apr: f64, as_of: i64) -> Self {
        Self {
            apr_24h: apr,
            apr_7d: apr,
            apr_30d: apr,
            as_of,
        }
    }

    /// From Phoenix's rate history: (unix seconds, percent paid that interval). The interval is
    /// read from the points (an hour on Phoenix). A window with no points takes all of them: a
    /// young market is scored on the history it has. None without any points.
    pub fn from_rates(points: &[(i64, f64)]) -> Option<Self> {
        let mut points = points.to_vec();
        points.sort_by_key(|p| p.0);
        let last = points.last()?.0;
        let mut gaps: Vec<i64> = points.windows(2).map(|w| w[1].0 - w[0].0).filter(|g| *g > 0).collect();
        gaps.sort_unstable();
        let interval = gaps.get(gaps.len() / 2).copied().unwrap_or(3_600) as f64;
        let per_year = YEAR_SECONDS / interval;
        let apr = |days: i64| {
            let window: Vec<f64> = points
                .iter()
                .filter(|p| p.0 > last - days * DAY)
                .map(|p| p.1)
                .collect();
            let window = if window.is_empty() {
                points.iter().map(|p| p.1).collect()
            } else {
                window
            };
            window.iter().sum::<f64>() / window.len() as f64 * per_year
        };
        Some(Self {
            apr_24h: apr(1),
            apr_7d: apr(7),
            apr_30d: apr(30),
            as_of: last,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hourly_rates_are_annualized_per_window() {
        // 30 days of hourly points: 0.001% an hour for 23 days, then 0.003% for the last week.
        let now = 1_800_000_000;
        let points: Vec<(i64, f64)> = (0..720)
            .map(|h| {
                let t = now - (719 - h) * 3_600;
                (t, if h >= 720 - 168 { 0.003 } else { 0.001 })
            })
            .collect();
        let f = Funding::from_rates(&points).unwrap();
        assert!((f.apr_24h - 0.003 * 8_760.0).abs() < 1e-9, "{f:?}");
        assert!((f.apr_7d - 0.003 * 8_760.0).abs() < 1e-9);
        let month = (552.0 * 0.001 + 168.0 * 0.003) / 720.0 * 8_760.0;
        assert!((f.apr_30d - month).abs() < 1e-9);
        assert!((f.score() - (f.apr_7d + f.apr_30d) / 2.0).abs() < 1e-12);
        assert_eq!(f.as_of, now);
    }

    #[test]
    fn a_young_market_is_scored_on_what_it_has() {
        let points = [(1_000, -0.002), (4_600, -0.002)];
        let f = Funding::from_rates(&points).unwrap();
        assert!((f.apr_30d + 0.002 * 8_760.0).abs() < 1e-9);
        assert!(Funding::from_rates(&[]).is_none());
    }
}
