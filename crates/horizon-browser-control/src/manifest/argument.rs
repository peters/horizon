//! One reader for a tool argument that a caller sends as its value or as the
//! JSON text of it, so an agent program that builds every argument as a string
//! still reaches an object or a list field.

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

/// `deserialize_with` of a field that a caller may send as its value or as the
/// JSON text of it. A text that holds no JSON stays a string, so the reader of
/// the field refuses it with the words of the field.
pub fn value_or_text<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = match Value::deserialize(deserializer)? {
        Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        value => value,
    };
    T::deserialize(value).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::block_terminal::TerminalRoute;

    fn route(json: &str) -> Result<TerminalRoute, serde_json::Error> {
        value_or_text(&mut serde_json::Deserializer::from_str(json))
    }

    #[test]
    fn a_value_stays_itself_and_the_text_of_a_value_is_the_value() {
        let object = route(r#"{"host": "lab-1", "port": 2222}"#).expect("the value itself");
        let text = route(r#""{\"host\": \"lab-1\", \"port\": 2222}""#).expect("the text of the value");
        assert_eq!((object.host.as_str(), object.port), ("lab-1", Some(2222)));
        assert_eq!(text, object);
        assert!(
            route(r#""not a route""#).is_err(),
            "a text that is no value of the field is still refused"
        );
        assert!(
            route(r#""{\"user\": \"deploy\"}""#).is_err(),
            "the text passes every check of the field"
        );
    }
}
