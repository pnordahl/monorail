use std::fmt;

#[derive(Debug)]
pub enum ErrorKind {
    Basic(String),
    Regex(regex::Error),
    Io(std::io::Error, String),
    Toml(toml::de::Error, String),
}
impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ErrorKind::Basic(message) => write!(f, "{}", message),
            ErrorKind::Regex(err) => write!(f, "{}", err.to_string()),
            ErrorKind::Io(err, target) => write!(f, "{}, target: {}", err.to_string(), target),
            ErrorKind::Toml(err, contents) => {
                write!(f, "{}, contents: {}", err.to_string(), contents)
            }
        }
    }
}
