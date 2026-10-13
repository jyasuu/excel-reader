//! Cell values used throughout the query engine.

use std::cmp::Ordering;
use std::fmt;

/// A single cell value. Numbers are always stored as `f64`; integral values are
/// rendered without a decimal part.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Number(f64),
    Text(String),
    Bool(bool),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Ranking used to give a total order across mixed types (for ORDER BY).
    fn type_rank(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Number(_) => 2,
            Value::Text(_) => 3,
        }
    }

    /// Best-effort numeric view of this value, coercing numeric strings.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Text(t) => t.trim().parse::<f64>().ok(),
            Value::Null => None,
        }
    }

    /// Compare two values, preferring a natural comparison when the types
    /// allow it and falling back to comparing numeric strings as numbers.
    pub fn compare(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Number(a), Value::Number(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
            (Value::Text(a), Value::Text(b)) => a.cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::Number(_), Value::Text(_)) | (Value::Text(_), Value::Number(_)) => {
                match (self.as_number(), other.as_number()) {
                    (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                    _ => self.type_rank().cmp(&other.type_rank()),
                }
            }
            _ => self.type_rank().cmp(&other.type_rank()),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => Ok(()),
            Value::Number(n) => {
                if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
                    write!(f, "{}", *n as i64)
                } else {
                    write!(f, "{n}")
                }
            }
            Value::Text(t) => write!(f, "{t}"),
            Value::Bool(b) => write!(f, "{b}"),
        }
    }
}
