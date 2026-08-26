use serde::{Deserialize, Deserializer, Serializer};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub mod opt {
    pub use super::{deserialize_opt_time as deserialize, serialize_opt_time as serialize};
}

pub fn deserialize_opt_time<'de, D>(deserializer: D) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    match raw.as_deref() {
        None | Some("") => Ok(None),
        Some(s) if s.starts_with("0001-01-01") => Ok(None),
        Some(s) => parse_rfc3339(s).map(Some).map_err(serde::de::Error::custom),
    }
}

pub fn serialize_opt_time<S>(
    value: &Option<OffsetDateTime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        None => serializer.serialize_none(),
        Some(dt) => {
            let s = dt.format(&Rfc3339).map_err(serde::ser::Error::custom)?;
            serializer.serialize_str(&s)
        }
    }
}

fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(s, &Rfc3339).or_else(|_| {
        let trimmed = s.trim_end_matches('Z');
        OffsetDateTime::parse(&format!("{trimmed}+00:00"), &Rfc3339)
    })
}
