use anyhow::Result;
use async_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::server::{Callback, ErrorResponse, Request as TungsteniteRequest, Response as TungsteniteResponse},
        http::StatusCode,
    },
};
use futures::{
    FutureExt, SinkExt, StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{App, AppContext, AsyncApp, Task};
use smol::net::{TcpListener, TcpStream};
use std::sync::Arc;

use crate::{
    listener::{McpDispatch, RawRequest},
    types::Request,
};

/// WebSocket MCP server.
///
/// Binds a TCP listener on localhost and accepts WebSocket connections,
/// routing JSON-RPC messages through `McpDispatch`. External agents discover
/// this server via the lock file written by `IdeLockFile`.
pub struct WsMcpServer {
    port: u16,
    auth_token: Arc<str>,
    dispatch: McpDispatch,
    _server_task: Task<()>,
}

impl WsMcpServer {
    pub fn new(cx: &AsyncApp) -> Task<Result<Self>> {
        let task = cx.background_spawn(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let port = listener.local_addr()?.port();
            let auth_token = generate_auth_token();
            anyhow::Ok((listener, port, auth_token))
        });

        cx.spawn(async move |cx| {
            let (listener, port, auth_token) = task.await?;
            let dispatch = McpDispatch::new();
            let dispatch_for_loop = dispatch.clone();
            let auth: Arc<str> = auth_token.into();
            let auth_for_loop = auth.clone();

            let server_task = cx.spawn({
                async move |cx| {
                    while let Ok((stream, _)) = listener.accept().await {
                        serve_ws_connection(
                            stream,
                            dispatch_for_loop.clone(),
                            auth_for_loop.clone(),
                            cx,
                        );
                    }
                }
            });

            Ok(Self {
                port,
                auth_token: auth,
                dispatch,
                _server_task: server_task,
            })
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }

    pub fn add_tool<T: crate::listener::McpServerTool + Clone + 'static>(&mut self, tool: T) {
        self.dispatch.add_tool(tool);
    }

    pub fn handle_request<R: Request>(
        &mut self,
        f: impl Fn(R::Params, &App) -> Task<Result<R::Response>> + 'static,
    ) {
        self.dispatch.handle_request::<R>(f);
    }
}

fn generate_auth_token() -> String {
    use std::fmt::Write as _;
    let bytes: [u8; 16] = rand::random();
    let mut token = String::with_capacity(32);
    for b in bytes {
        write!(token, "{b:02x}").ok();
    }
    token
}

fn serve_ws_connection(
    tcp_stream: TcpStream,
    dispatch: McpDispatch,
    auth_token: Arc<str>,
    cx: &mut AsyncApp,
) {
    let (incoming_tx, incoming_rx) = unbounded::<RawRequest>();
    let (outgoing_tx, outgoing_rx) = unbounded::<String>();

    // Spawn IO (WS upgrade + frame loop) on background thread. Only uses
    // channels and the TcpStream — no Rc types.
    cx.background_spawn(async move {
        let ws_stream = async_tungstenite::accept_hdr_async(
            tcp_stream,
            AuthCallback { auth_token },
        )
        .await;

        match ws_stream {
            Ok(ws) => {
                if let Err(e) = handle_ws_io(outgoing_rx, incoming_tx, ws).await {
                    log::debug!("WS connection closed: {e}");
                }
            }
            Err(e) => {
                log::warn!("WS handshake failed: {e}");
            }
        }
    })
    .detach();

    // Dispatch loop runs on main thread (accesses Rc<RefCell<...>> tool registry).
    dispatch.dispatch_connection(incoming_rx, outgoing_tx, cx);
}

struct AuthCallback {
    auth_token: Arc<str>,
}

impl Callback for AuthCallback {
    fn on_request(
        self,
        request: &TungsteniteRequest,
        response: TungsteniteResponse,
    ) -> std::result::Result<TungsteniteResponse, ErrorResponse> {
        let authorized = request
            .uri()
            .query()
            .and_then(|q| {
                q.split('&')
                    .find(|p| p.starts_with("authToken="))
                    .map(|p| &p["authToken=".len()..])
            })
            .map(|token| token == self.auth_token.as_ref())
            .unwrap_or(false);

        if authorized {
            Ok(response)
        } else {
            let mut error_resp = ErrorResponse::new(Some("Unauthorized".to_string()));
            *error_resp.status_mut() = StatusCode::UNAUTHORIZED;
            Err(error_resp)
        }
    }
}

async fn handle_ws_io(
    mut outgoing_rx: UnboundedReceiver<String>,
    incoming_tx: UnboundedSender<RawRequest>,
    ws_stream: WebSocketStream<TcpStream>,
) -> Result<()> {
    let (mut ws_sink, mut ws_source): (
        async_tungstenite::WebSocketSender<TcpStream>,
        async_tungstenite::WebSocketReceiver<TcpStream>,
    ) = ws_stream.split();

    loop {
        futures::select_biased! {
            message = outgoing_rx.next().fuse() => {
                match message {
                    Some(msg) => {
                        log::trace!("ws send: {}", &msg);
                        ws_sink.send(Message::Text(msg.into())).await?;
                    }
                    None => break,
                }
            }
            frame = ws_source.next().fuse() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        log::trace!("ws recv: {}", text.as_str());
                        match serde_json::from_str(text.as_str()) {
                            Ok(request) => {
                                incoming_tx.unbounded_send(request).ok();
                            }
                            Err(e) => {
                                log::error!("failed to parse WS message: {e}");
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ignore ping/pong/binary frames
                    Some(Err(e)) => {
                        log::debug!("WS error: {e}");
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
