use serde::{Deserialize, Serialize};
use std::{fmt, str};

use std::collections::HashSet;
use std::result::Result;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::{debug, error, info, instrument};

#[derive(Debug)]
pub enum ServerError {
    Filter(String),
    LogClient(std::io::Error),
    LogServer(std::io::Error),
    BindTimeout(tokio::time::error::Elapsed),
    Lock(std::io::Error),
}
impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::LogServer(e) => {
                write!(f, "Log server error: {}", e)
            }
            ServerError::LogClient(e) => {
                write!(f, "Log client error: {}", e)
            }
            ServerError::Filter(e) => {
                write!(f, "Invalid log filter: {}", e)
            }
            ServerError::BindTimeout(e) => {
                write!(f, "Bind timed out: {}", e)
            }
            ServerError::Lock(e) => {
                write!(f, "Lock acquisition failed: {}", e)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogServerConfig {
    #[serde(default = "default_host")]
    pub(crate) host: String,
    #[serde(default = "LogServerConfig::default_port")]
    pub(crate) port: usize,
    #[serde(default = "default_bind_timeout_ms")]
    pub(crate) bind_timeout_ms: u64,
}
impl LogServerConfig {
    pub(crate) fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
    fn default_port() -> usize {
        5918
    }
}
impl Default for LogServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: Self::default_port(),
            bind_timeout_ms: default_bind_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LogFilterInput {
    pub(crate) commands: HashSet<String>,
    pub(crate) targets: HashSet<String>,
    pub(crate) include_stdout: bool,
    pub(crate) include_stderr: bool,
}

pub(crate) struct LogServer<'a> {
    config: LogServerConfig,
    address: String,
    bind_timeout: std::time::Duration,
    filter: &'a LogFilterInput,
}
impl<'a> LogServer<'a> {
    pub(crate) fn new(config: LogServerConfig, filter: &'a LogFilterInput) -> Self {
        let address = config.address();
        let bind_timeout = std::time::Duration::from_millis(config.bind_timeout_ms);
        Self {
            config,
            address,
            bind_timeout,
            filter,
        }
    }
    pub(crate) async fn serve(self) -> Result<(), ServerError> {
        info!(
            address = &self.address,
            timeout = &self.config.bind_timeout_ms,
            "Log server binding"
        );
        let timeout_res = tokio::time::timeout(
            self.bind_timeout,
            tokio::net::TcpListener::bind(&self.address),
        )
        .await;
        match timeout_res {
            Ok(Ok(listener)) => {
                info!("Log server accepting clients");
                let mut args_data = serde_json::to_vec(&self.filter)
                    .map_err(|e| ServerError::Filter(e.to_string()))?;
                args_data.push(b'\n');
                loop {
                    let (mut socket, _) =
                        listener.accept().await.map_err(ServerError::LogServer)?;
                    debug!("Log server client connected");
                    // first, write to the client what we're interested in receiving
                    socket
                        .write_all(&args_data)
                        .await
                        .map_err(ServerError::LogServer)?;
                    debug!("Sent log stream arguments to client");
                    Self::process(socket).await?;
                }
            }
            Ok(Err(e)) => {
                error!(address = &self.address, "Log server bind failed");
                Err(ServerError::Lock(e))
            }
            Err(e) => Err(ServerError::BindTimeout(e)),
        }
    }
    #[instrument]
    async fn process(socket: tokio::net::TcpStream) -> Result<(), ServerError> {
        let br = tokio::io::BufReader::new(socket);
        let mut lines = br.lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await.map_err(ServerError::LogClient)? {
            stdout
                .write_all(line.as_bytes())
                .await
                .map_err(ServerError::LogClient)?;
            _ = stdout.write(b"\n").await.map_err(ServerError::LogClient)?;
            stdout.flush().await.map_err(ServerError::LogClient)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockServerConfig {
    #[serde(default = "default_host")]
    pub(crate) host: String,
    #[serde(default = "LockServerConfig::default_port")]
    pub(crate) port: usize,
    #[serde(default = "default_bind_timeout_ms")]
    pub(crate) bind_timeout_ms: u64,
}
impl LockServerConfig {
    fn default_port() -> usize {
        5917
    }
}
impl Default for LockServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: Self::default_port(),
            bind_timeout_ms: default_bind_timeout_ms(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_bind_timeout_ms() -> u64 {
    1000
}

// The lock server prevents concurrent use of a subset of monorail
// APIs, such as `run` and `checkpoint update`.
// A server bind is used instead of a lock file, because it's more
// robust and easier to use than an ephemeral file that requires
// pid tracking and manual cleanup. This technique essentially defers
// that responsibility to the OS, which already performs these tasks.
pub(crate) struct LockServer {
    config: LockServerConfig,
    address: String,
    bind_timeout: std::time::Duration,
    listener: Option<tokio::net::TcpListener>,
}
impl LockServer {
    pub(crate) fn new(config: LockServerConfig) -> Self {
        let address = format!("{}:{}", config.host, config.port);
        let bind_timeout = std::time::Duration::from_millis(config.bind_timeout_ms);
        Self {
            config,
            address,
            bind_timeout,
            listener: None,
        }
    }
    pub(crate) async fn acquire(mut self) -> Result<Self, ServerError> {
        info!(
            address = &self.address,
            timeout = &self.config.bind_timeout_ms,
            "Acquiring lock"
        );
        let timeout_res = tokio::time::timeout(
            self.bind_timeout,
            tokio::net::TcpListener::bind(&self.address),
        )
        .await;
        match timeout_res {
            Ok(Ok(listener)) => {
                info!("Lock acquired");
                self.listener = Some(listener);
                Ok(self)
            }
            Ok(Err(e)) => {
                error!(address = &self.address, "Lock failed");
                Err(ServerError::Lock(e))
            }
            Err(e) => Err(ServerError::BindTimeout(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use tokio::sync::Mutex;

    static TEST_MUTEX: Lazy<Mutex<i32>> = Lazy::new(|| Mutex::new(0));

    #[tokio::test]
    async fn test_bind_acquire_success() {
        let _guard = TEST_MUTEX.lock().await;
        let config: LockServerConfig = Default::default();
        let server = LockServer::new(config).acquire().await;
        assert!(server.is_ok());
        assert!(server.unwrap().listener.is_some());
    }

    #[tokio::test]
    async fn test_bind_error() {
        let config = LockServerConfig {
            host: "192.0.2.0".to_string(), // Non-routeable ip
            port: 12345,
            bind_timeout_ms: 100000,
        };
        let result = LockServer::new(config).acquire().await;
        assert!(matches!(result, Err(ServerError::Lock(_))));
    }

    #[tokio::test]
    async fn test_acquire() {
        let _guard = TEST_MUTEX.lock().await;
        // Bind the port with one server
        let config1: LockServerConfig = Default::default();
        let guard1 = LockServer::new(config1.clone()).acquire().await;
        assert!(guard1.is_ok());

        // Try binding with another server in Lock mode
        let guard2 = LockServer::new(config1).acquire().await;
        assert!(matches!(guard2, Err(ServerError::Lock(_))));
    }
}
