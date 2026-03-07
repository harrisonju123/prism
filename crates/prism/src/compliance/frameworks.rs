use serde::Serialize;

/// Supported compliance frameworks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Framework {
    Soc2,
    Iso27001,
    Hipaa,
}

impl Framework {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "soc2" => Some(Framework::Soc2),
            "iso27001" => Some(Framework::Iso27001),
            "hipaa" => Some(Framework::Hipaa),
            _ => None,
        }
    }

    pub fn required_sections(&self) -> &'static [&'static str] {
        match self {
            Framework::Soc2 => &[
                "access_controls",
                "data_encryption",
                "audit_logging",
                "change_management",
                "incident_response",
            ],
            Framework::Iso27001 => &[
                "access_controls",
                "data_classification",
                "audit_logging",
                "risk_assessment",
                "business_continuity",
            ],
            Framework::Hipaa => &[
                "access_controls",
                "data_encryption",
                "audit_logging",
                "minimum_necessary",
                "breach_notification",
            ],
        }
    }
}

/// A compliance report section.
#[derive(Debug, Clone, Serialize)]
pub struct ComplianceSection {
    pub name: String,
    pub status: ComplianceStatus,
    pub details: String,
    pub evidence_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceStatus {
    Compliant,
    Partial,
    NonCompliant,
    NotApplicable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_roundtrip() {
        assert_eq!(Framework::from_str("soc2"), Some(Framework::Soc2));
        assert_eq!(Framework::from_str("iso27001"), Some(Framework::Iso27001));
        assert_eq!(Framework::from_str("hipaa"), Some(Framework::Hipaa));
        assert_eq!(Framework::from_str("unknown"), None);
    }

    #[test]
    fn soc2_has_required_sections() {
        let sections = Framework::Soc2.required_sections();
        assert!(sections.contains(&"access_controls"));
        assert!(sections.contains(&"audit_logging"));
    }
}
