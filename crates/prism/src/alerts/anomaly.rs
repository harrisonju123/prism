use super::types::AlertSeverity;

#[derive(Debug, Clone)]
pub struct AnomalyResult {
    pub is_anomaly: bool,
    pub z_score: f64,
    pub severity: AlertSeverity,
    pub message: String,
}

pub fn compute_z_score(current: f64, mean: f64, stddev: f64) -> f64 {
    if stddev == 0.0 || stddev.is_nan() {
        return 0.0;
    }
    (current - mean) / stddev
}

/// Score today's spend against a rolling baseline.
/// Z > 2.0 -> warning, Z > 3.0 -> critical.
pub fn classify_spend_anomaly(
    current_day_spend: f64,
    baseline_mean: f64,
    baseline_stddev: f64,
    warning_z: f64,
    critical_z: f64,
) -> AnomalyResult {
    let z = compute_z_score(current_day_spend, baseline_mean, baseline_stddev);

    if z > critical_z {
        AnomalyResult {
            is_anomaly: true,
            z_score: z,
            severity: AlertSeverity::Critical,
            message: format!(
                "Spend anomaly (critical): ${current_day_spend:.2} today vs \
                 ${baseline_mean:.2} avg (z={z:.2})"
            ),
        }
    } else if z > warning_z {
        AnomalyResult {
            is_anomaly: true,
            z_score: z,
            severity: AlertSeverity::Warning,
            message: format!(
                "Spend anomaly (warning): ${current_day_spend:.2} today vs \
                 ${baseline_mean:.2} avg (z={z:.2})"
            ),
        }
    } else {
        AnomalyResult {
            is_anomaly: false,
            z_score: z,
            severity: AlertSeverity::Info,
            message: "Spend within normal range".into(),
        }
    }
}

/// Flag when failure rate spikes relative to the baseline.
pub fn detect_failure_rate_spike(
    current_rate: f64,
    baseline_rate: f64,
    spike_threshold: f64,
    min_failure_rate: f64,
) -> Option<AnomalyResult> {
    if current_rate < min_failure_rate {
        return None;
    }

    if baseline_rate <= 0.0 {
        if current_rate >= min_failure_rate {
            return Some(AnomalyResult {
                is_anomaly: true,
                z_score: 0.0,
                severity: AlertSeverity::Warning,
                message: format!(
                    "Failure rate spike: {:.1}% (baseline had no failures)",
                    current_rate * 100.0
                ),
            });
        }
        return None;
    }

    let ratio = current_rate / baseline_rate;
    if ratio >= spike_threshold {
        let severity = if ratio >= 3.0 {
            AlertSeverity::Critical
        } else {
            AlertSeverity::Warning
        };
        Some(AnomalyResult {
            is_anomaly: true,
            z_score: 0.0,
            severity,
            message: format!(
                "Failure rate spike: {:.1}% vs {:.1}% baseline ({ratio:.1}x)",
                current_rate * 100.0,
                baseline_rate * 100.0
            ),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_score_zero_stddev_returns_zero() {
        assert!((compute_z_score(100.0, 50.0, 0.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn z_score_normal() {
        let z = compute_z_score(60.0, 50.0, 5.0);
        assert!((z - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn spend_anomaly_critical() {
        let result = classify_spend_anomaly(100.0, 50.0, 10.0, 2.0, 3.0);
        assert!(result.is_anomaly);
        assert_eq!(result.severity, AlertSeverity::Critical);
        assert!((result.z_score - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn spend_anomaly_warning() {
        let result = classify_spend_anomaly(75.0, 50.0, 10.0, 2.0, 3.0);
        assert!(result.is_anomaly);
        assert_eq!(result.severity, AlertSeverity::Warning);
    }

    #[test]
    fn spend_within_normal_range() {
        let result = classify_spend_anomaly(55.0, 50.0, 10.0, 2.0, 3.0);
        assert!(!result.is_anomaly);
        assert_eq!(result.severity, AlertSeverity::Info);
    }

    #[test]
    fn failure_spike_detected() {
        let result = detect_failure_rate_spike(0.2, 0.05, 2.0, 0.05);
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.is_anomaly);
        assert_eq!(r.severity, AlertSeverity::Critical); // 4x > 3x
    }

    #[test]
    fn failure_below_min_rate_ignored() {
        let result = detect_failure_rate_spike(0.01, 0.005, 2.0, 0.05);
        assert!(result.is_none());
    }

    #[test]
    fn failure_no_baseline_flags_new_failures() {
        let result = detect_failure_rate_spike(0.1, 0.0, 2.0, 0.05);
        assert!(result.is_some());
    }
}
