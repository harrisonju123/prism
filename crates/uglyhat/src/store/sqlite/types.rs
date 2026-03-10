use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{Error, Result};

macro_rules! row_to_struct {
    (
        $vis:vis fn $fn_name:ident ($row:ident) -> $struct_name:ident {
            $( $field:ident : $kind:ident $col:literal $(=> $custom:expr)? ),* $(,)?
        }
    ) => {
        $vis fn $fn_name($row: &sqlx::sqlite::SqliteRow) -> $crate::error::Result<$struct_name> {
            use sqlx::Row as _;
            Ok($struct_name {
                $( $field: row_to_struct!(@extract $row, $kind, $col $(, $custom)? ), )*
            })
        }
    };

    (@extract $row:ident, uuid, $col:literal) => {
        parse_uuid(&$row.try_get::<String, _>($col)?)?
    };
    (@extract $row:ident, opt_uuid, $col:literal) => {
        parse_opt_uuid($row.try_get::<Option<String>, _>($col)?)?
    };
    (@extract $row:ident, time, $col:literal) => {
        parse_time(&$row.try_get::<String, _>($col)?)?
    };
    (@extract $row:ident, opt_time, $col:literal) => {
        parse_opt_time($row.try_get::<Option<String>, _>($col)?)?
    };
    (@extract $row:ident, json_array, $col:literal) => {
        parse_json_array(&$row.try_get::<String, _>($col)?)
    };
    (@extract $row:ident, opt_json, $col:literal) => {
        str_to_opt_value($row.try_get::<Option<String>, _>($col)?)
    };
    (@extract $row:ident, str, $col:literal) => {
        $row.try_get($col)?
    };
    (@extract $row:ident, bool, $col:literal) => {
        $row.try_get($col)?
    };
    (@extract $row:ident, f64, $col:literal) => {
        $row.try_get::<f64, _>($col)?
    };
    (@extract $row:ident, opt_f64, $col:literal) => {
        $row.try_get::<Option<f64>, _>($col)?
    };
    (@extract $row:ident, custom, $col:literal, $custom:expr) => {
        $custom
    };
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn parse_uuid(s: &str) -> Result<Uuid> {
    s.parse::<Uuid>()
        .map_err(|e| Error::Internal(format!("invalid uuid {s:?}: {e}")))
}

pub fn parse_time(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("invalid timestamp {s:?}: {e}")))
}

pub fn parse_opt_uuid(s: Option<String>) -> Result<Option<Uuid>> {
    match s {
        Some(ref v) if !v.is_empty() => Ok(Some(parse_uuid(v)?)),
        _ => Ok(None),
    }
}

pub fn parse_opt_time(s: Option<String>) -> Result<Option<DateTime<Utc>>> {
    match s {
        Some(ref v) if !v.is_empty() => Ok(Some(parse_time(v)?)),
        _ => Ok(None),
    }
}

pub fn parse_json_array(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

pub fn json_array_to_str(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}

pub fn opt_value_to_str(v: &Option<serde_json::Value>) -> Option<String> {
    v.as_ref()
        .map(|val| serde_json::to_string(val).unwrap_or_default())
}

pub fn str_to_opt_value(s: Option<String>) -> Option<serde_json::Value> {
    s.and_then(|v| serde_json::from_str(&v).ok())
}

pub fn opt_uuid_to_str(u: Option<Uuid>) -> Option<String> {
    u.map(|id| id.to_string())
}

pub fn push_tag_clauses(tags: &[String], clauses: &mut Vec<String>, args: &mut Vec<String>) {
    for tag in tags {
        args.push(tag.clone());
        clauses.push(format!(
            "EXISTS (SELECT 1 FROM json_each(tags) WHERE json_each.value = ${})",
            args.len()
        ));
    }
}
