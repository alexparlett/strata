//! The MCP server: Streamable HTTP on loopback, bearer token, **stop on drop**.
//!
//! The Engine pattern, for the Engine's reason: rmcp needs a Tokio reactor and the UI thread is not
//! one, so [`AgentServer`] owns a small private runtime and the caller holds a plain handle.
//!
//! **Why HTTP and not a Unix socket:** the transport menu belongs to the client. MCP clients speak
//! stdio — where the *client* spawns the server, structurally impossible inside an already-running
//! GUI — and Streamable HTTP; a UDS server would force a stdio↔socket proxy into every connection.
//!
//! rmcp's `StreamableHttpService` is a tower service with no listener of its own, which is the seam
//! the auth check sits in: [`serve`] answers 401 itself and only then hands the request over, so an
//! unauthorized call cannot reach the router, let alone a `Host`.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1::Builder as Http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio::runtime::{Builder as RuntimeBuilder, Runtime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::host::Host;
use crate::tools::{StrataTools, STATELESS_IDLE};

/// The path the MCP endpoint is served at — the one an `mcp add --transport http` URL ends
/// with.
pub const MCP_PATH: &str = "/mcp";

/// The auth scheme, lowercased for the case-insensitive compare RFC 7235 asks for.
const BEARER: &str = "bearer ";

/// How often [`sweep`] looks, **derived from the window it enforces**.
///
/// A sweep can only retract on a tick, so the interval is what turns [`STATELESS_IDLE`] into
/// the bound the tool description and the system prompt state: half the window puts retraction
/// between one and one and a half idle windows after the last call. Named rather than written
/// into the `interval` call so it cannot be pinned to a number while the window moves.
///
/// `checked_div` rather than `/`, which is not `const` — and rather than halving `as_secs`,
/// which would truncate a sub-second window to a zero interval, the one period
/// [`tokio::time::interval`] panics on.
pub const SWEEP_INTERVAL: Duration = STATELESS_IDLE.checked_div(2).expect("2 is not zero");

/// Mint a bearer token for [`AgentServer::start`].
///
/// Here rather than beside the setting it is stored in, because the rule it has to satisfy is
/// this file's: [`authorized`] is a byte compare, so the one token that must never be produced
/// is the empty one, and the refusal that enforces that is ten lines up. A `Uuid::new_v4`
/// hyphenless is 122 bits of randomness in 32 URL-safe characters — long enough that guessing
/// it is not a threat model, short enough to paste into a shell command.
///
/// Minting is deliberate and its result is **persisted**: a token regenerated per launch would
/// invalidate the client configuration the user pasted last time.
pub fn mint_token() -> String {
    Uuid::new_v4().simple().to_string()
}

/// How long the accept loop waits after a failed `accept()` before trying again.
///
/// Not politeness — a fd exhaustion (`EMFILE`) makes `accept` fail *immediately and
/// repeatedly*, so a bare `continue` pegs a core and floods the log for as long as the
/// pressure lasts, which is precisely when the app can least afford either. Short enough that
/// a genuinely transient refusal costs nothing.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(50);

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
    /// Retract every stateless agent this server minted, run from [`Drop`].
    ///
    /// A boxed closure rather than a `StrataTools<H>` field, so `AgentServer` stays free of
    /// the `H` parameter every caller would otherwise have to name. It cannot live in the
    /// sweep task instead: `Drop` calls `shutdown_background`, which drops the runtime's
    /// tasks rather than polling them, so anything placed after the loop's `break` would
    /// simply never run.
    retract: Box<dyn Fn() + Send>,
}

impl AgentServer {
    /// Bind `127.0.0.1:port` and serve `host`'s projects to any client presenting `token`.
    ///
    /// Binding happens before this returns, so a taken port is an error the caller can show
    /// rather than a server that silently never listens. `port` may be `0`, in which case
    /// [`addr`](AgentServer::addr) reports the one the OS chose.
    ///
    /// An **empty token is refused**, and loudly: the guard is a byte compare, so an empty
    /// secret matches a bare `Authorization: Bearer ` and every local process is authorized.
    /// A server nobody can reach is a bug the user reports; a server everybody can reach is
    /// one nobody notices.
    pub fn start<H: Host>(port: u16, token: String, host: Arc<H>) -> Result<AgentServer, String> {
        if token.is_empty() {
            return Err("agent server needs a token: an empty one authorizes every request".into());
        }
        let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .map_err(|e| format!("agent server could not bind 127.0.0.1:{port}: {e}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("agent server listener: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("agent server address: {e}"))?;
        let rt = RuntimeBuilder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("strata-agent")
            .build()
            .map_err(|e| format!("agent server runtime: {e}"))?;

        let cancel = CancellationToken::new();
        let tools = StrataTools::new(host);
        let sweeper = tools.clone();
        let service = StreamableHttpService::new(
            move || Ok(tools.connection()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default().with_cancellation_token(cancel.clone()),
        );
        rt.spawn(accept(listener, service, Arc::new(token), cancel.clone()));
        rt.spawn(sweep(sweeper.clone(), cancel.clone()));

        tracing::info!("agent access listening on http://{addr}{MCP_PATH}");
        Ok(AgentServer {
            rt: Some(rt),
            cancel,
            addr,
            retract: Box::new(move || sweeper.retire_all()),
        })
    }

    /// Where it is listening — the address a client configuration names.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        (self.retract)();
        self.cancel.cancel();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// Retract stateless agents that have gone quiet.
///
/// **The one poll in this crate, and it is here because nothing on our side can observe the
/// fact**: a client on MCP's discover lifecycle has no session, so its
/// departure produces no close, no `DELETE` and no value whose drop means anything — see
/// [`StrataTools::retire_idle`]. It exists only while the server does, which is the other
/// half of that rule: the timer is not a standing cost of the app, it is a cost of having the
/// feature on.
///
/// Swept at [`SWEEP_INTERVAL`], so an agent is retracted between one and one and a half
/// [`STATELESS_IDLE`]s after its last call rather than up to two.
async fn sweep<H: Host>(tools: StrataTools<H>, cancel: CancellationToken) {
    let mut ticks = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = ticks.tick() => tools.retire_idle(STATELESS_IDLE),
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
            () = cancel.cancelled() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::warn!("agent server accept failed: {e}");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(ACCEPT_BACKOFF) => continue,
                    }
                }
            },
        };
        let service = service.clone();
        let token = Arc::clone(&token);
        tokio::spawn(async move {
            let handler = service_fn(move |req: Request<Incoming>| {
                let service = service.clone();
                let token = Arc::clone(&token);
                async move { Ok::<_, Infallible>(serve(&service, &token, req).await) }
            });
            if let Err(e) = Http1::new()
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
/// The **scheme** is matched case-insensitively, because RFC 7235 says it is a case-insensitive
/// token and a client sending `bearer` is presenting a valid credential — 401-ing it would send
/// the user off to re-mint a token that was right all along. The **secret** is compared in
/// constant time: overkill for a loopback socket, and free, which is the point — a comparison
/// that short-circuits on the first wrong byte is the kind of thing nobody revisits when the
/// same code is later reachable from somewhere else.
///
/// An empty `token` cannot reach here: [`AgentServer::start`] refuses one, because this compare
/// would accept a bare `Authorization: Bearer ` against it.
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let Some(value) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some((scheme, presented)) = value.split_at_checked(BEARER.len()) else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case(BEARER) {
        return false;
    }
    let (a, b) = (presented.as_bytes(), token.as_bytes());
    if a.len() != b.len() || b.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unauthorized() -> Response<Body> {
    let mut response = plain(StatusCode::UNAUTHORIZED, "Unauthorized");
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn plain(status: StatusCode, message: &'static str) -> Response<Body> {
    let mut response = Response::new(Full::new(Bytes::from_static(message.as_bytes())).boxed());
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    /// **The sweep looks often enough for the stated bound to be true.** `retire_idle` can only
    /// act on a tick, so a client is retracted up to one interval *after* its window closes —
    /// which is the difference between the 30 minutes the tool description and the system
    /// prompt state and a client kept for an hour because somebody left the interval at a
    /// literal while the window moved.
    #[test]
    fn the_sweep_ticks_inside_the_idle_window() {
        assert!(
            !SWEEP_INTERVAL.is_zero(),
            "a zero interval is a busy loop, not a sweep"
        );
        assert!(
            SWEEP_INTERVAL <= STATELESS_IDLE / 2,
            "retraction lands within one and a half idle windows"
        );
    }

    #[test]
    fn the_right_token_is_authorized() {
        assert!(authorized(&bearer("Bearer s3cret"), "s3cret"));
    }

    /// RFC 7235's auth-scheme is a case-insensitive token, so `bearer` is a valid credential
    /// and 401-ing it sends the user off to re-mint a token that was already right.
    #[test]
    fn the_scheme_is_case_insensitive_but_the_secret_is_not() {
        assert!(authorized(&bearer("bearer s3cret"), "s3cret"));
        assert!(authorized(&bearer("BEARER s3cret"), "s3cret"));
        assert!(!authorized(&bearer("Bearer S3CRET"), "s3cret"));
    }

    /// The guard is a byte compare, so an empty secret would match a bare `Bearer `.
    /// `AgentServer::start` refuses one; this is the second half of that rule, in the
    /// comparison itself.
    #[test]
    fn an_empty_token_authorizes_nobody() {
        assert!(!authorized(&bearer("Bearer "), ""));
        assert!(!authorized(&bearer("Bearer"), ""));
        assert!(!authorized(&HeaderMap::new(), ""));
    }

    #[test]
    fn a_wrong_missing_or_malformed_token_is_not() {
        assert!(!authorized(&bearer("Bearer nope"), "s3cret"));
        assert!(!authorized(&bearer("Bearer s3cre"), "s3cret"));
        assert!(!authorized(&bearer("s3cret"), "s3cret"));
        assert!(!authorized(&HeaderMap::new(), "s3cret"));
    }
}
