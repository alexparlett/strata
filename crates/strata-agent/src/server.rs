//! The MCP server: Streamable HTTP on loopback, bearer token, **stop on drop**.
//!
//! The Engine pattern, for the same reason the engine uses it. rmcp needs a Tokio reactor
//! and the app's UI thread is not one, so [`AgentServer`] owns a small private runtime and
//! the caller holds a plain handle. Nothing about starting or stopping it asks the app to
//! own an executor.
//!
//! **Why HTTP and not a Unix socket:** the transport menu belongs to the client. MCP clients
//! speak stdio (where the *client* spawns the server — structurally impossible for a server
//! living inside an already-running GUI) and Streamable HTTP. A UDS server would force a
//! stdio↔socket proxy into every connection. So: loopback bind plus a bearer token, checked
//! before the request reaches a tool.
//!
//! rmcp's `StreamableHttpService` is a tower service with no listener of its own, which is
//! the seam the auth check sits in: [`serve`] answers 401 itself and only then hands the
//! request over, so an unauthorized call cannot reach the router, let alone a `Host`.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;

use bytes::Bytes;
use http::{header, HeaderMap, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio::runtime::{Builder, Runtime};
use tokio_util::sync::CancellationToken;

use crate::host::Host;
use crate::tools::StrataTools;

/// The path the MCP endpoint is served at — the one an `mcp add --transport http` URL ends
/// with.
pub const MCP_PATH: &str = "/mcp";

type Body = BoxBody<Bytes, Infallible>;
type Mcp<H> = StreamableHttpService<StrataTools<H>, LocalSessionManager>;

/// A running agent-access server. Dropping it stops the listener, terminates every live MCP
/// session and shuts the runtime down — there is no `stop()` to forget to call.
pub struct AgentServer {
    /// `Option` only so `Drop` can take the runtime for a context-safe
    /// `shutdown_background`: dropping a `Runtime` from inside another runtime panics, and a
    /// caller may well be one. Always `Some` while the server lives.
    rt: Option<Runtime>,
    cancel: CancellationToken,
    addr: SocketAddr,
}

impl AgentServer {
    /// Bind `127.0.0.1:port` and serve `host`'s projects to any client presenting `token`.
    ///
    /// Binding happens before this returns, so a taken port is an error the caller can show
    /// rather than a server that silently never listens. `port` may be `0`, in which case
    /// [`addr`](AgentServer::addr) reports the one the OS chose.
    pub fn start<H: Host>(port: u16, token: String, host: Arc<H>) -> Result<AgentServer, String> {
        // Bound with `std` and **before** the runtime exists, for two separate reasons that
        // both bite a caller who is already inside a runtime: `rt.block_on` panics there,
        // and so does dropping a `Runtime` — which is what an early `?` on a taken port
        // would do if the runtime were built first. Nothing about claiming a port is async;
        // the spawned task adopts the listener with `from_std`, in the runtime's context.
        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .map_err(|e| format!("agent server could not bind 127.0.0.1:{port}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("agent server listener: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("agent server address: {e}"))?;
        let rt = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("strata-agent")
            .build()
            .map_err(|e| format!("agent server runtime: {e}"))?;

        let cancel = CancellationToken::new();
        let tools = StrataTools::new(host);
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            Arc::new(LocalSessionManager::default()),
            // Defaults throughout but the token: the DNS-rebinding host allow-list already
            // names loopback, and session mode is left as rmcp ships it so clients that
            // negotiate an older protocol version still pair.
            StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone()),
        );
        rt.spawn(accept(listener, service, Arc::new(token), cancel.clone()));

        tracing::info!("agent access listening on http://{addr}{MCP_PATH}");
        Ok(AgentServer {
            rt: Some(rt),
            cancel,
            addr,
        })
    }

    /// Where it is listening — the address a client configuration names.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

async fn accept<H: Host>(
    listener: StdTcpListener,
    service: Mcp<H>,
    token: Arc<String>,
    cancel: CancellationToken,
) {
    let listener = match TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("agent server could not adopt its listener: {e}");
            return;
        }
    };
    loop {
        let stream = tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                // One refused connection is not a reason to stop listening.
                Err(e) => {
                    tracing::warn!("agent server accept failed: {e}");
                    continue;
                }
            },
        };
        let service = service.clone();
        let token = Arc::clone(&token);
        tokio::spawn(async move {
            let handler = hyper::service::service_fn(move |req: Request<Incoming>| {
                let service = service.clone();
                let token = Arc::clone(&token);
                async move { Ok::<_, Infallible>(serve(&service, &token, req).await) }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), handler)
                .await
            {
                tracing::debug!("agent connection ended: {e}");
            }
        });
    }
}

/// One request: authorize, route, hand over.
async fn serve<H: Host>(service: &Mcp<H>, token: &str, req: Request<Incoming>) -> Response<Body> {
    if !authorized(req.headers(), token) {
        return unauthorized();
    }
    if req.uri().path() != MCP_PATH {
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    service.handle(req).await
}

/// Does the request carry `Authorization: Bearer <token>`?
///
/// Compared in constant time. Overkill for a loopback socket, and free — a comparison that
/// short-circuits on the first wrong byte is the kind of thing nobody revisits when the same
/// code is later reachable from somewhere else.
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(presented) = value.strip_prefix("Bearer ") else {
        return false;
    };
    let (a, b) = (presented.as_bytes(), token.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized() -> Response<Body> {
    let mut response = plain(StatusCode::UNAUTHORIZED, "Unauthorized");
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        header::HeaderValue::from_static("Bearer"),
    );
    response
}

fn plain(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from_static(message.as_bytes())).boxed());
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn the_right_token_is_authorized() {
        assert!(authorized(&bearer("Bearer s3cret"), "s3cret"));
    }

    #[test]
    fn a_wrong_missing_or_malformed_token_is_not() {
        assert!(!authorized(&bearer("Bearer nope"), "s3cret"));
        assert!(!authorized(&bearer("Bearer s3cre"), "s3cret"));
        assert!(!authorized(&bearer("s3cret"), "s3cret"));
        assert!(!authorized(&HeaderMap::new(), "s3cret"));
    }
}
