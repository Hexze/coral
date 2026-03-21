mod auth;
mod codes;
mod connection;
mod encryption;
mod protocol;

use std::sync::Arc;

use base64::Engine;
use connection::ServerState;
use encryption::ServerKey;
use tokio::net::TcpListener;
use tracing::info;

pub use codes::VerifiedPlayer;

const DEFAULT_MOTD: &str = "Coral Account Linking\nJoin and copy the provided 4-digit code";
const DEFAULT_ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

type FormatFn = Box<dyn Fn(&str) -> String + Send + Sync>;

pub struct VerifyServer {
    address: String,
    disconnect_message: Option<FormatFn>,
}

#[derive(Clone)]
pub struct VerifyHandle {
    state: Arc<ServerState>,
}

impl VerifyServer {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            disconnect_message: None,
        }
    }

    pub fn disconnect_message<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        self.disconnect_message = Some(Box::new(f));
        self
    }

    pub async fn start(self) -> std::io::Result<VerifyHandle> {
        info!("generating RSA keypair...");
        let state = Arc::new(ServerState {
            key: ServerKey::generate(),
            http: reqwest::Client::new(),
            codes: codes::CodeStore::new(),
            motd: DEFAULT_MOTD.into(),
            server_icon: Some(base64::engine::general_purpose::STANDARD.encode(DEFAULT_ICON_PNG)),
            format_disconnect: self.disconnect_message.unwrap_or_else(|| {
                Box::new(|code| {
                    format!("Your verification code is: §a§l{code}\n\n§7Expires in 2 minutes.")
                })
            }),
        });

        let listener = TcpListener::bind(&self.address).await?;
        info!("verify server listening on {}", self.address);

        let handle = VerifyHandle {
            state: Arc::clone(&state),
        };

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let state = Arc::clone(&state);
                        tokio::spawn(connection::handle_connection(stream, state));
                    }
                    Err(e) => tracing::error!("accept failed: {e}"),
                }
            }
        });

        Ok(handle)
    }
}

impl VerifyHandle {
    pub fn redeem(&self, code: &str) -> Option<VerifiedPlayer> {
        self.state.codes.redeem(code)
    }
}
