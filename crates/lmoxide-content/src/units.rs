//! Values people write by hand: durations ("1.5s", "200ms", "2m", or seconds), ratios ("20%",
//! or a fraction), and number ranges (`[1, 3]` or one number). Each parses either way and is
//! written back in one canonical form, so files diff cleanly.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A length of time, in seconds. Written as `"1.5s"`, `"200ms"`, `"2m"`, `"1h"` or a number of
/// seconds; written back as seconds (`"1.5s"`), or `"ms"` below a second.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Duration(pub f64);

impl Duration {
    pub fn secs(self) -> f64 {
        self.0
    }

    pub fn to_std(self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.0.max(0.0))
    }
}

impl std::str::FromStr for Duration {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (number, scale) = if let Some(n) = s.strip_suffix("ms") {
            (n, 0.001)
        } else if let Some(n) = s.strip_suffix('s') {
            (n, 1.0)
        } else if let Some(n) = s.strip_suffix('m') {
            (n, 60.0)
        } else if let Some(n) = s.strip_suffix('h') {
            (n, 3600.0)
        } else {
            (s, 1.0)
        };
        let n: f64 = number
            .trim()
            .parse()
            .map_err(|_| format!("not a duration: {s:?} (try \"1.5s\" or \"200ms\")"))?;
        Ok(Duration(n * scale))
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0 != 0.0 && self.0.abs() < 1.0 {
            write!(f, "{}ms", round(self.0 * 1000.0))
        } else {
            write!(f, "{}s", round(self.0))
        }
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match NumberOrText::deserialize(d)? {
            NumberOrText::Number(n) => Ok(Duration(n)),
            NumberOrText::Text(t) => t.parse().map_err(de::Error::custom),
        }
    }
}

impl JsonSchema for Duration {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "x-lmoxide-unit": "duration",
            "description": "A length of time: \"1.5s\", \"200ms\", \"2m\", \"1h\", or a number of seconds",
            "anyOf": [
                { "type": "string", "pattern": "^\\s*-?[0-9.]+\\s*(ms|s|m|h)?\\s*$" },
                { "type": "number" }
            ]
        })
    }
}

/// A share: `"20%"` or a fraction (`0.2`); written back as a percentage.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Ratio(pub f64);

impl Ratio {
    pub fn fraction(self) -> f64 {
        self.0
    }
}

impl std::str::FromStr for Ratio {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let bad = || format!("not a share: {s:?} (try \"20%\" or 0.2)");
        match s.strip_suffix('%') {
            Some(n) => n
                .trim()
                .parse::<f64>()
                .map(|n| Ratio(n / 100.0))
                .map_err(|_| bad()),
            None => s.parse().map(Ratio).map_err(|_| bad()),
        }
    }
}

impl fmt::Display for Ratio {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}%", round(self.0 * 100.0))
    }
}

impl Serialize for Ratio {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Ratio {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match NumberOrText::deserialize(d)? {
            NumberOrText::Number(n) => Ok(Ratio(n)),
            NumberOrText::Text(t) => t.parse().map_err(de::Error::custom),
        }
    }
}

impl JsonSchema for Ratio {
    fn schema_name() -> Cow<'static, str> {
        "Ratio".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "x-lmoxide-unit": "ratio",
            "description": "A share: \"20%\" or a fraction (0.2)",
            "anyOf": [
                { "type": "string", "pattern": "^\\s*-?[0-9.]+\\s*%\\s*$" },
                { "type": "number" }
            ]
        })
    }
}

/// A whole-number range: `[min, max]` or one number (min = max).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub min: u32,
    pub max: u32,
}

impl Range {
    pub fn one(n: u32) -> Self {
        Range { min: n, max: n }
    }
}

impl Serialize for Range {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.min == self.max {
            s.serialize_u32(self.min)
        } else {
            [self.min, self.max].serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for Range {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum R {
            One(u32),
            Two([u32; 2]),
        }
        Ok(match R::deserialize(d)? {
            R::One(n) => Range::one(n),
            R::Two([min, max]) if min <= max => Range { min, max },
            R::Two(_) => return Err(de::Error::custom("range [min, max] needs min <= max")),
        })
    }
}

impl JsonSchema for Range {
    fn schema_name() -> Cow<'static, str> {
        "Range".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "x-lmoxide-unit": "range",
            "description": "A number, or [min, max]",
            "anyOf": [
                { "type": "integer", "minimum": 0 },
                { "type": "array", "items": { "type": "integer", "minimum": 0 }, "minItems": 2, "maxItems": 2 }
            ]
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrText {
    Number(f64),
    Text(String),
}

/// Up to 6 decimals without float noise (0.1 + 0.2 shows as 0.3).
fn round(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        for (text, secs, back) in [
            ("1.5s", 1.5, "\"1.5s\""),
            ("200ms", 0.2, "\"200ms\""),
            ("2m", 120.0, "\"120s\""),
            ("3", 3.0, "\"3s\""),
        ] {
            let d: Duration = serde_json::from_str(&format!("\"{text}\"")).unwrap();
            assert_eq!(d.secs(), secs);
            assert_eq!(serde_json::to_string(&d).unwrap(), back);
        }
        let d: Duration = serde_json::from_str("0.25").unwrap();
        assert_eq!(serde_json::to_string(&d).unwrap(), "\"250ms\"");
        assert!(serde_json::from_str::<Duration>("\"soon\"").is_err());
    }

    #[test]
    fn ratios() {
        let r: Ratio = serde_json::from_str("\"20%\"").unwrap();
        assert_eq!(r.fraction(), 0.2);
        let r: Ratio = serde_json::from_str("0.035").unwrap();
        assert_eq!(serde_json::to_string(&r).unwrap(), "\"3.5%\"");
    }

    #[test]
    fn ranges() {
        let r: Range = serde_json::from_str("[1, 3]").unwrap();
        assert_eq!((r.min, r.max), (1, 3));
        assert_eq!(serde_json::to_string(&Range::one(2)).unwrap(), "2");
        assert!(serde_json::from_str::<Range>("[3, 1]").is_err());
    }
}
