use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tracing::{debug, warn};

use crate::errors::SlurpError;

// Wire format is `{ "type": "reload", "pages": [...] }` - matches the TypeScript client expectations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HmrMessage {
    Reload { pages: Vec<String> },
    Error { errors: Vec<SlurpError> },
    Clear,
}

// Lagging subscribers receive RecvError::Lagged and are sent a full reload.
const BROADCAST_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct HmrHub {
    sender: Arc<broadcast::Sender<HmrMessage>>,
    client_count: Arc<Mutex<usize>>,
}

impl HmrHub {
    #[must_use]
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            sender: Arc::new(sender),
            client_count: Arc::new(Mutex::new(0)),
        }
    }

    pub async fn broadcast(&self, msg: HmrMessage) -> usize {
        match serde_json::to_string(&msg) {
            Ok(json) => {
                // send() errors only when there are zero subscribers - normal when no tabs are open.
                match self.sender.send(msg) {
                    Ok(n) => {
                        debug!(count = n, payload = %json, "HMR broadcast sent");
                        n
                    }
                    Err(_) => {
                        debug!("HMR broadcast: no subscribers");
                        0
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize HMR message: {e}");
                0
            }
        }
    }

    pub async fn handle_socket(&self, socket: WebSocket, client_addr: std::net::SocketAddr) {
        let mut rx = self.sender.subscribe();

        {
            let mut count = self.client_count.lock().await;
            *count += 1;
            debug!(addr = %client_addr, total = *count, "HMR client connected");
        }

        let client_count = Arc::clone(&self.client_count);

        tokio::spawn(async move {
            let (mut ws_tx, mut ws_rx) = socket.split();

            loop {
                tokio::select! {
                    result = rx.recv() => {
                        match result {
                            Ok(msg) => {
                                match serde_json::to_string(&msg) {
                                    Ok(json) => {
                                        if ws_tx.send(Message::Text(json)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!("HMR serialize error: {e}");
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(
                                    addr = %client_addr,
                                    skipped,
                                    "HMR client lagged; sending full reload"
                                );
                                let reload = HmrMessage::Reload { pages: vec!["*".to_string()] };
                                if let Ok(json) = serde_json::to_string(&reload)
                                    && ws_tx.send(Message::Text(json)).await.is_err()
                                {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }

                    // Must drain incoming frames to detect clean close (clients send nothing meaningful).
                    frame = ws_rx.next() => {
                        match frame {
                            Some(Ok(Message::Close(_))) | None => break,
                            Some(Ok(_)) => {}
                            Some(Err(e)) => {
                                debug!(addr = %client_addr, "HMR WS receive error: {e}");
                                break;
                            }
                        }
                    }
                }
            }

            let mut count = client_count.lock().await;
            *count = count.saturating_sub(1);
            debug!(addr = %client_addr, total = *count, "HMR client disconnected");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format is a contract with `runtime/src/core/dev.ts`, which switches on
    /// `msg.type` behind a `never` exhaustiveness guard. A rename on either side is silent:
    /// the Rust `#[serde(rename_all = "camelCase")]` would emit a tag the client's `default`
    /// arm logs and ignores, so the overlay would simply stop working.
    #[test]
    fn every_message_serializes_to_the_tag_the_browser_client_switches_on() {
        let clear = serde_json::to_string(&HmrMessage::Clear).expect("serialize");
        assert_eq!(clear, r#"{"type":"clear"}"#);

        let reload = serde_json::to_string(&HmrMessage::Reload {
            pages: vec!["/index".to_string()],
        })
        .expect("serialize");
        assert_eq!(reload, r#"{"type":"reload","pages":["/index"]}"#);

        let error =
            serde_json::to_string(&HmrMessage::Error { errors: vec![] }).expect("serialize");
        assert_eq!(error, r#"{"type":"error","errors":[]}"#);
    }
}
