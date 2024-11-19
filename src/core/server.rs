use serde::{Deserialize, Serialize};
use std::{fmt, str};
use tracing::{error, info};

#[derive(Debug)]
pub enum ServerError {
    BindTimeout(tokio::time::error::Elapsed),
    Lock(std::io::Error),
}
impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::BindTimeout(e) => {
                write!(f, "Bind timed out: {}", e)
            }
            ServerError::Lock(e) => {
                write!(f, "Lock acquisition failed: {}", e)
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServerConfig {
    pub(crate) lock: LockServerConfig,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LockServerConfig {
    #[serde(default = "LockServerConfig::default_host")]
    pub(crate) host: String,
    #[serde(default = "LockServerConfig::default_port")]
    pub(crate) port: usize,
    #[serde(default = "LockServerConfig::default_bind_timeout_ms")]
    pub(crate) bind_timeout_ms: u64,
}
impl LockServerConfig {
    fn default_host() -> String {
        "127.0.0.1".to_string()
    }
    fn default_port() -> usize {
        5917
    }
    fn default_bind_timeout_ms() -> u64 {
        1000
    }
}
impl Default for LockServerConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            bind_timeout_ms: Self::default_bind_timeout_ms(),
        }
    }
}

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
