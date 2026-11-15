//! The summary of a timed series: its median, its 99th percentile by nearest rank, its longest.

use crate::json::Json;

#[derive(Clone, Copy)]
pub struct Summary {
    pub count: usize,
    pub median: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    /// The summary of `values`, or `None` when there are none.
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        let median = (sorted[(count - 1) / 2] + sorted[count / 2]) / 2.0;
        // Nearest rank: the smallest value that at least 99 % of the series is at or below.
        let rank = (count as f64 * 0.99).ceil() as usize;
        let p99 = sorted[rank.clamp(1, count) - 1];
        Some(Self {
            count,
            median,
            p99,
            max: sorted[count - 1],
        })
    }

    pub fn to_json(self) -> Json {
        Json::Object(vec![
            ("count", Json::Integer(self.count as i64)),
            ("median", Json::Number(self.median)),
            ("p99", Json::Number(self.p99)),
            ("max", Json::Number(self.max)),
        ])
    }

    /// `median / p99 / max`, with one decimal.
    pub fn to_cell(self) -> String {
        format!("{:.1} / {:.1} / {:.1}", self.median, self.p99, self.max)
    }
}

/// A summary's JSON, or null.
pub fn summary_json(summary: Option<Summary>) -> Json {
    Json::optional(summary, Summary::to_json)
}

/// A summary's cell, or a dash.
pub fn summary_cell(summary: Option<Summary>) -> String {
    summary.map_or_else(|| "-".to_owned(), Summary::to_cell)
}

#[cfg(test)]
mod tests {
    use super::Summary;

    #[test]
    fn takes_the_median_and_the_nearest_rank() {
        let values: Vec<f64> = (1..=200).map(f64::from).rev().collect();
        let summary = Summary::of(&values).expect("values are given");
        assert_eq!(summary.median, 100.5);
        assert_eq!(summary.p99, 198.0);
        assert_eq!(summary.max, 200.0);
        let one = Summary::of(&[7.0]).expect("one value");
        assert_eq!((one.median, one.p99, one.max), (7.0, 7.0, 7.0));
        assert!(Summary::of(&[]).is_none());
    }
}
