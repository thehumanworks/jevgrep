use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

/// Text or structured JSON: what the API reference writes as `string | object | array`, and
/// TypeSafe's own SDKs call `JSONContent`.
///
/// Instructions, the descriptions of a [`Choice`](crate::Choice)'s options, a
/// [`Score`](crate::Score)'s levels and a [`Noul`](crate::Noul)'s two outcomes all take it. Text
/// is the common case, and a `&str` or `String` converts with `into()`. Structure is for a
/// question that refers to data of its own: put the data in one field and the question in
/// another, and name the data field in backticks.
///
/// ```
/// use serde_json::json;
/// use typesafe_jev::Content;
///
/// let text: Content = "Is the resume for the same person?".into();
/// assert_eq!(text.as_str(), Some("Is the resume for the same person?"));
///
/// let structured = Content::try_from(json!({
///     "potential_duplicate": {"name": "John Smith", "location": "Oakland, California"},
///     "question": "Is the resume for the same person as `potential_duplicate`?",
/// }))?;
/// assert!(matches!(structured, Content::Object(_)));
///
/// // A number, a boolean or null is not content.
/// assert!(Content::try_from(json!(42)).is_err());
/// # Ok::<(), typesafe_jev::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, expecting = "a string, an object or an array")]
pub enum Content {
    /// Plain text.
    Text(String),
    /// A JSON object, whose fields the text of a question can name in backticks.
    Object(Map<String, Value>),
    /// A JSON array.
    Array(Vec<Value>),
}

impl Content {
    /// The text, if this is [`Content::Text`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Content::Text(text) => Some(text),
            Content::Object(_) | Content::Array(_) => None,
        }
    }
}

impl From<&str> for Content {
    fn from(text: &str) -> Self {
        Content::Text(text.to_owned())
    }
}

impl From<String> for Content {
    fn from(text: String) -> Self {
        Content::Text(text)
    }
}

impl From<&String> for Content {
    fn from(text: &String) -> Self {
        Content::Text(text.clone())
    }
}

impl From<Map<String, Value>> for Content {
    fn from(object: Map<String, Value>) -> Self {
        Content::Object(object)
    }
}

impl From<Vec<Value>> for Content {
    fn from(array: Vec<Value>) -> Self {
        Content::Array(array)
    }
}

impl TryFrom<Value> for Content {
    type Error = Error;

    /// [`Error::InvalidRequest`] for a number, a boolean or null.
    fn try_from(value: Value) -> Result<Self, Error> {
        match value {
            Value::String(text) => Ok(Content::Text(text)),
            Value::Object(object) => Ok(Content::Object(object)),
            Value::Array(array) => Ok(Content::Array(array)),
            Value::Null | Value::Bool(_) | Value::Number(_) => {
                Err(Error::InvalidRequest("content must be a string, an object or an array".into()))
            }
        }
    }
}

impl From<Content> for Value {
    fn from(content: Content) -> Self {
        match content {
            Content::Text(text) => Value::String(text),
            Content::Object(object) => Value::Object(object),
            Content::Array(array) => Value::Array(array),
        }
    }
}
