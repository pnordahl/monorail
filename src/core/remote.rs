use serde::{Deserialize, Serialize};
use std::{fmt, str};
use tracing::{error, info};

#[derive(Debug)]
pub enum RemoteError {
    BindTimeout(tokio::time::error::Elapsed),
    Lock,
    Serve(std::io::Error),
}
impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteError::BindTimeout(e) => {
                write!(f, "Bind timed out: {}", e)
            }
            RemoteError::Lock => {
                write!(f, "Another process holds the server lock")
            }
            RemoteError::Serve(e) => {
                write!(f, "Remote server bind failed: {}", e)
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteConfig {
    pub(crate) server: RemoteServerConfig,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteServerConfig {
    #[serde(default = "RemoteServerConfig::default_host")]
    pub(crate) host: String,
    #[serde(default = "RemoteServerConfig::default_port")]
    pub(crate) port: usize,
    #[serde(default = "RemoteServerConfig::default_bind_timeout_ms")]
    pub(crate) bind_timeout_ms: u64,
}
impl RemoteServerConfig {
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
impl Default for RemoteServerConfig {
    fn default() -> Self {
        Self {
            host: Self::default_host(),
            port: Self::default_port(),
            bind_timeout_ms: Self::default_bind_timeout_ms(),
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) enum RemoteServerMode {
    // Run in serve mode, accepting remote requests for run, log, etc.
    #[serde(rename = "serve")]
    Serve,
    // Lock-only mode, binding the server port and preventing
    // multiple instances of monorail.
    #[default]
    #[serde(rename = "lock")]
    Lock,
}

pub(crate) struct RemoteServer {
    config: RemoteServerConfig,
    address: String,
    bind_timeout: std::time::Duration,
    listener: Option<tokio::net::TcpListener>,
    mode: RemoteServerMode,
}
impl RemoteServer {
    pub(crate) fn new(config: RemoteServerConfig, mode: RemoteServerMode) -> Self {
        let address = format!("{}:{}", config.host, config.port);
        let bind_timeout = std::time::Duration::from_millis(config.bind_timeout_ms);
        Self {
            config,
            address,
            bind_timeout,
            listener: None,
            mode,
        }
    }
    // Convenience function for returning a pre-locked server.
    pub(crate) async fn new_lock(config: RemoteServerConfig) -> Result<Self, RemoteError> {
        let mut rs = Self::new(config, RemoteServerMode::Lock);
        rs.bind().await?;
        Ok(rs)
    }
    pub(crate) async fn bind(&mut self) -> Result<(), RemoteError> {
        info!(
            address = &self.address,
            timeout = &self.config.bind_timeout_ms,
            "Binding"
        );
        let timeout_res = tokio::time::timeout(
            self.bind_timeout,
            tokio::net::TcpListener::bind(&self.address),
        )
        .await;
        match timeout_res {
            Ok(Ok(listener)) => {
                info!("Bind successful");
                self.listener = Some(listener);
                Ok(())
            }
            Ok(Err(e)) => match &self.mode {
                RemoteServerMode::Lock => {
                    error!(address = &self.address, "Address lock already held");
                    Err(RemoteError::Lock)
                }
                RemoteServerMode::Serve => Err(RemoteError::Serve(e)),
            },
            Err(e) => Err(RemoteError::BindTimeout(e)),
        }
    }
    // TODO: serve implementation
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use tokio::sync::Mutex;

    static TEST_MUTEX: Lazy<Mutex<i32>> = Lazy::new(|| Mutex::new(0));

    #[tokio::test]
    async fn test_bind_success() {
        let config = RemoteServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // Let the OS assign a free port
            bind_timeout_ms: 1000,
        };
        let mut server = RemoteServer::new(config, RemoteServerMode::Serve);
        assert!(server.bind().await.is_ok());
        assert!(server.listener.is_some());
    }

    #[tokio::test]
    async fn test_bind_error() {
        let config = RemoteServerConfig {
            host: "192.0.2.0".to_string(), // Non-routeable ip
            port: 12345,
            bind_timeout_ms: 100000,
        };
        let mut server = RemoteServer::new(config, RemoteServerMode::Serve);
        let result = server.bind().await;
        assert!(matches!(result, Err(RemoteError::Serve(_))));
    }

    #[tokio::test]
    async fn test_bind_lock_mode() {
        let _guard = TEST_MUTEX.lock().await;
        // Bind the port with one server
        let config1 = RemoteServerConfig {
            host: "127.0.0.1".to_string(),
            port: 9999,
            bind_timeout_ms: 1000,
        };
        let mut server1 = RemoteServer::new(config1.clone(), RemoteServerMode::Lock);
        assert!(server1.bind().await.is_ok());

        // Try binding with another server in Lock mode
        let mut server2 = RemoteServer::new(config1, RemoteServerMode::Lock);
        let result = server2.bind().await;
        dbg!(&result);
        assert!(matches!(result, Err(RemoteError::Lock)));
    }

    #[tokio::test]
    async fn test_bind_serve_mode_failure() {
        let _guard = TEST_MUTEX.lock().await;
        // Bind the port with one server
        let config1 = RemoteServerConfig {
            host: "127.0.0.1".to_string(),
            port: 9999,
            bind_timeout_ms: 1000,
        };
        let mut server1 = RemoteServer::new(config1.clone(), RemoteServerMode::Serve);
        assert!(server1.bind().await.is_ok());

        // Try binding with another server in Serve mode
        let mut server2 = RemoteServer::new(config1, RemoteServerMode::Serve);
        let result = server2.bind().await;
        dbg!(&result);
        if let Err(RemoteError::Serve(e)) = result {
            assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse);
        } else {
            panic!("Expected RemoteError::Serve");
        }
    }
}
