use serde::Serialize;
use std::error::Error;
use std::fmt;

#[derive(Debug, Serialize, Eq, PartialEq)]
pub enum ErrorClass {
    #[serde(rename = "generic")]
    Generic,
    #[serde(rename = "git2")]
    Git2,
    #[serde(rename = "io")]
    Io,
    #[serde(rename = "toml_deserialize")]
    TomlDeserialize,
    #[serde(rename = "serde_json")]
    SerdeJSON,
    #[serde(rename = "utf8")]
    Utf8Error,
    #[serde(rename = "parse_int")]
    ParseIntError,
    #[serde(rename = "dependency_graph")]
    DependencyGraph,
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct MonorailError {
    pub class: ErrorClass,
    pub message: String,
}
impl Error for MonorailError {}
impl fmt::Display for MonorailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "class: {:?}, message: {}", self.class, self.message)
    }
}
impl From<git2::Error> for MonorailError {
    fn from(error: git2::Error) -> Self {
        MonorailError {
            class: ErrorClass::Git2,
            message: format!(
                "class: {:?}, code: {:?}, message: {:?}",
                error.class(),
                error.code(),
                error.message()
            ),
        }
    }
}
impl From<&str> for MonorailError {
    fn from(error: &str) -> Self {
        MonorailError {
            class: ErrorClass::Generic,
            message: error.to_string(),
        }
    }
}
impl From<String> for MonorailError {
    fn from(error: String) -> Self {
        MonorailError {
            class: ErrorClass::Generic,
            message: error,
        }
    }
}
impl From<std::io::Error> for MonorailError {
    fn from(error: std::io::Error) -> Self {
        MonorailError {
            class: ErrorClass::Io,
            message: error.to_string(),
        }
    }
}
impl From<std::str::Utf8Error> for MonorailError {
    fn from(error: std::str::Utf8Error) -> Self {
        MonorailError {
            class: ErrorClass::Utf8Error,
            message: error.to_string(),
        }
    }
}
impl From<toml::de::Error> for MonorailError {
    fn from(error: toml::de::Error) -> Self {
        MonorailError {
            class: ErrorClass::TomlDeserialize,
            message: error.to_string(),
        }
    }
}
impl From<serde_json::error::Error> for MonorailError {
    fn from(error: serde_json::error::Error) -> Self {
        MonorailError {
            class: ErrorClass::SerdeJSON,
            message: error.to_string(),
        }
    }
}
impl From<std::num::ParseIntError> for MonorailError {
    fn from(error: std::num::ParseIntError) -> Self {
        MonorailError {
            class: ErrorClass::ParseIntError,
            message: error.to_string(),
        }
    }
}
