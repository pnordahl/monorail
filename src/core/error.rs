use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use std::{fmt, io, num, str};

#[derive(Debug, Serialize)]
pub enum GraphError {
    LabelNotFound(usize),
    Cycle(usize, String),
    DuplicateLabel(String),
    LabelNodeNotFound(String),
}
impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::LabelNotFound(node) => {
                write!(f, "Label not found for node: {}", node)
            }
            GraphError::Cycle(node, label) => {
                write!(f, "Cycle detected at node: {}, label: {}", node, label)
            }
            GraphError::DuplicateLabel(label) => {
                write!(f, "Duplicate label provided: {}", label)
            }
            GraphError::LabelNodeNotFound(label) => {
                write!(f, "Node not found for label: {}", label)
            }
        }
    }
}

#[derive(Debug)]
pub enum MonorailError {
    Generic(String),
    Git(String),
    Io(io::Error),
    PathDNE(String),
    SerdeJSON(serde_json::error::Error),
    Utf8(str::Utf8Error),
    ParseInt(num::ParseIntError),
    DependencyGraph(GraphError),
    Join(tokio::task::JoinError),
    TrackingCheckpointNotFound(io::Error),
    TrackingRunNotFound(io::Error),
    MissingArg(String),
    TaskCancelled,
    ChannelSend(String),
    ChannelRecv(flume::RecvError),
}

impl From<flume::RecvError> for MonorailError {
    fn from(error: flume::RecvError) -> Self {
        MonorailError::ChannelRecv(error)
    }
}
impl<T> From<flume::SendError<T>> for MonorailError {
    fn from(error: flume::SendError<T>) -> Self {
        MonorailError::ChannelSend(error.to_string())
    }
}
impl From<String> for MonorailError {
    fn from(error: String) -> Self {
        MonorailError::Generic(error)
    }
}
impl From<&str> for MonorailError {
    fn from(error: &str) -> Self {
        MonorailError::Generic(error.to_owned())
    }
}
impl From<std::io::Error> for MonorailError {
    fn from(error: std::io::Error) -> Self {
        MonorailError::Io(error)
    }
}
impl From<std::str::Utf8Error> for MonorailError {
    fn from(error: std::str::Utf8Error) -> Self {
        MonorailError::Utf8(error)
    }
}
impl From<serde_json::error::Error> for MonorailError {
    fn from(error: serde_json::error::Error) -> Self {
        MonorailError::SerdeJSON(error)
    }
}
impl From<std::num::ParseIntError> for MonorailError {
    fn from(error: std::num::ParseIntError) -> Self {
        MonorailError::ParseInt(error)
    }
}
impl From<tokio::task::JoinError> for MonorailError {
    fn from(error: tokio::task::JoinError) -> Self {
        MonorailError::Join(error)
    }
}

impl fmt::Display for MonorailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MonorailError::Generic(error) => write!(f, "Error; {}", error),
            MonorailError::Git(error) => write!(f, "Git error; {}", error),
            MonorailError::Io(error) => write!(f, "IO error; {}", error),
            MonorailError::PathDNE(error) => write!(f, "Path does not exist: {}", error),
            MonorailError::SerdeJSON(error) => write!(f, "JSON error; {}", error),
            MonorailError::Utf8(error) => write!(f, "UTF8 error; {}", error),
            MonorailError::ParseInt(error) => write!(f, "Integer parsing error; {}", error),
            MonorailError::DependencyGraph(error) => {
                write!(f, "Dependency graph error; {}", error)
            }
            MonorailError::Join(error) => write!(f, "Task join error; {}", error),
            MonorailError::MissingArg(s) => write!(f, "Missing argument error; {}", s),
            MonorailError::TrackingCheckpointNotFound(error) => {
                write!(f, "Tracking checkpoint open error; {}", error)
            }
            MonorailError::TrackingRunNotFound(error) => {
                write!(f, "Tracking log info open error; {}", error)
            }
            MonorailError::TaskCancelled => {
                write!(f, "Task cancelled")
            }
            MonorailError::ChannelSend(error) => {
                write!(f, "Channel send error; {}", error)
            }
            MonorailError::ChannelRecv(error) => {
                write!(f, "Channel recv error; {}", error)
            }
        }
    }
}

impl Serialize for MonorailError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize the error as an object with "type" and "message" fields
        let mut state = serializer.serialize_struct("MonorailError", 2)?;
        state.serialize_field("kind", "error")?;

        match self {
            MonorailError::Generic(_) => {
                state.serialize_field("type", "generic")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::Git(_) => {
                state.serialize_field("type", "Git")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::Io(_) => {
                state.serialize_field("type", "io")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::SerdeJSON(_) => {
                state.serialize_field("type", "json")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::Utf8(_) => {
                state.serialize_field("type", "utf8")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::ParseInt(_) => {
                state.serialize_field("type", "parse_int")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::DependencyGraph(_) => {
                state.serialize_field("type", "dependency_graph")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::PathDNE(_) => {
                state.serialize_field("type", "path_dne")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::Join(_) => {
                state.serialize_field("type", "task_join")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::TrackingCheckpointNotFound(_) => {
                state.serialize_field("type", "tracking_checkpoint_not_found")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::TrackingRunNotFound(_) => {
                state.serialize_field("type", "tracking_log_info_not_found")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::MissingArg(_) => {
                state.serialize_field("type", "missing_arg")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::TaskCancelled => {
                state.serialize_field("type", "task_cancelled")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::ChannelSend(_) => {
                state.serialize_field("type", "channel_send")?;
                state.serialize_field("message", &self.to_string())?;
            }
            MonorailError::ChannelRecv(_) => {
                state.serialize_field("type", "channel_recv")?;
                state.serialize_field("message", &self.to_string())?;
            }
        }
        state.end()
    }
}

impl std::error::Error for MonorailError {}
