use async_tungstenite::tokio::connect_async;
use async_tungstenite::tokio::TokioAdapter;
use async_tungstenite::tungstenite::protocol::Message as WebSocketMessage;
use async_tungstenite::WebSocketStream;

use futures::SinkExt;
use rand::prelude::*;
use serde_json::Value;
use tokio_rustls;

use anyhow::{anyhow, bail, Context, Result};

use super::types::Bid;
use futures::StreamExt;
use thiserror::Error;
use tracing::{trace, warn};

pub enum SocketError {}

pub struct Socket {
    socket: WebSocketStream<
        async_tungstenite::stream::Stream<
            TokioAdapter<tokio::net::TcpStream>,
            TokioAdapter<tokio_rustls::client::TlsStream<tokio::net::TcpStream>>,
        >,
    >,
    message_counter: u32,
}

#[derive(Debug)]
pub enum Message {
    BidsUpdate {
        contract_addr: String,
        bids: Vec<Bid>,
    },
}

#[derive(Debug)]
pub enum Subscription {
    Collection { addr: String },
}

impl Socket {
    fn gen_id() -> String {
        const ALPHABET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const LENGTH: usize = 12;

        let mut result = String::new();

        for _ in 1..=LENGTH {
            let index = thread_rng().next_u32() % (ALPHABET.len() as u32);
            let char = ALPHABET
                .char_indices()
                .filter(|arg| arg.0 == index as usize)
                .next()
                .unwrap()
                .1;

            result += &char.to_string();
        }

        result
    }

    pub async fn new() -> Result<Socket> {
        let url = format!(
            "wss://feeds.prod.blur.io/socket.io/?tabId={}&storageId={}&EIO=4&transport=websocket",
            Self::gen_id(),
            Self::gen_id()
        );

        let (mut socket, _) = connect_async(url).await?;

        socket.send(WebSocketMessage::Text("40".to_owned())).await?; // socket io connect

        Ok(Socket {
            socket,
            message_counter: 0,
        })
    }

    fn try_parse_event(mut msg: String) -> Result<serde_json::Value> {
        let remove_range = {
            let chars = msg.chars();
            let mut last_digit = 0;

            for (index, char) in chars.enumerate() {
                if !('0'..='9').contains(&char) {
                    break;
                }
                last_digit = index;
            }

            0..last_digit + 1
        };

        msg.drain(remove_range);

        Ok(serde_json::from_str(&msg)?)
    }

    pub async fn poll(&mut self) -> Result<Option<Message>> {
        while let Some(msg) = self.socket.next().await {
            let WebSocketMessage::Text(msg) = msg? else {
                continue;
            };
            trace!(msg = msg, "socket.ws.recv");

            if msg.as_str() == "2" {
                self.socket
                    .send(WebSocketMessage::Text("3".to_owned()))
                    .await?;
                self.socket.flush().await?;
                trace!("socket.server_ping");
                continue;
            }
            // if not a engine.io message (message type prefix is 4)
            else if !msg.starts_with("4") {
                continue;
            }

            let event = Self::try_parse_event(msg.clone())
                .context("Failed parsing socket.io event")
                .with_context(|| msg.clone())?;

            // because socket io message is array
            let event = match event {
                Value::Array(event) => event,
                _ => continue,
            };

            let event_name = event
                .get(0)
                .ok_or(anyhow!(
                    "socket.io event doesn't contain event name at index 0"
                ))?
                .as_str()
                .ok_or(anyhow!("socket.io event name is not a string"))?;

            let event_data = event.get(1).ok_or(anyhow!(
                "socket.io event data doesn't contain data at index 1"
            ))?;

            return Ok(Some(match event_name {
                event_name if event_name.ends_with("COLLECTION.{}.denormalizer.bidLevels") => {
                    let raw_updates = event_data
                        .as_object()
                        .ok_or(anyhow!("event data is not an object"))?
                        .get("updates")
                        .ok_or(anyhow!("No updates field"))?
                        .as_array()
                        .ok_or(anyhow!("updates field is not an array"))?;
                    let data: Result<Vec<Bid>> = raw_updates
                        .iter()
                        .map(|x| {
                            serde_json::from_value(x.clone())
                                .map_err(move |e| anyhow::Error::from(e))
                        })
                        .collect();

                    trace!(data = ?data, "socket.event.bidUpdate");
                    Message::BidsUpdate {
                        contract_addr: event_data
                            .get("contractAddress")
                            .unwrap()
                            .as_str()
                            .unwrap()
                            .to_owned(),
                        bids: data?,
                    }
                }
                _ => {
                    warn!(socket.event = event_name, "socket.unrecognized_event_name");
                    continue;
                }
            }));
        }

        Ok(None)
    }

    pub async fn subscribe(&mut self, sub: Subscription) -> Result<()> {
        use Subscription::*;
        let msg = match sub {
            Collection { addr } => {
                format!(
                    r#"42{}["subscribe",["{}.COLLECTION.{{}}.denormalizer.bidLevels"]]"#,
                    self.message_counter, addr
                )
            }
        };

        // println!("SENT {msg}");
        trace!(msg = msg, "socket.ws.sent");

        self.socket.send(WebSocketMessage::Text(msg)).await?;
        self.socket.flush().await?;
        self.message_counter += 1;

        Ok(())
    }
}

use tokio::sync::mpsc;
pub struct AsyncSocket {
    task: Option<tokio::task::JoinHandle<Result<()>>>,
    rx: mpsc::Receiver<Message>,
}

impl AsyncSocket {
    pub async fn stream(&mut self) -> Result<AsyncSocketStream> {
        if self.task.is_none() {
            bail!("calling stream on broken socket");
        }
        let task = self.task.as_ref().unwrap();
        if task.is_finished() {
            let task = self.task.take().unwrap();
            match task.await.unwrap() {
                Ok(_) => {
                    bail!("tf is this error");
                }
                Err(err) => return Err(err),
            }
        }
        Ok(AsyncSocketStream { socket: self })
    }
}

pub struct AsyncSocketStream<'a> {
    socket: &'a mut AsyncSocket,
}

impl<'a> futures::Stream for AsyncSocketStream<'a> {
    type Item = Message;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().socket.rx.poll_recv(cx)
    }
}

const SOCKET_SIZE: usize = 1_000_000;
pub struct AsyncSocketBuilder {
    subs: Vec<Subscription>,
}
impl AsyncSocketBuilder {
    pub fn new() -> Self {
        Self { subs: Vec::new() }
    }

    pub fn subscribe(&mut self, sub: Subscription) {
        self.subs.push(sub)
    }

    pub fn build(self) -> AsyncSocket {
        let (tx, rx) = mpsc::channel(SOCKET_SIZE);

        let task = tokio::task::spawn(async move {
            let mut socket = Socket::new().await?;

            for coll in self.subs.into_iter() {
                socket.subscribe(coll).await?
            }

            loop {
                while let Some(msg) = socket.poll().await? {
                    tx.send(msg).await?;
                    tracing::trace!(event = "socket.async.recv");
                }
            }

            Ok(())
        });

        AsyncSocket {
            task: Some(task),
            rx,
        }
    }
}
