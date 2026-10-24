//! Discord's local IPC protocol. All transport and request handling lives in Rust.
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{broadcast, mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const NOT_CONNECTED: &str = "Discord is not connected.";
const DISCONNECTED: &str = "Discord disconnected.";
const HANDSHAKE_TIMEOUT: &str =
    "Discord did not respond. Check your application ID and restart Discord.";

trait Transport: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Transport for T {}
type IpcStream = Box<dyn Transport>;
type Endpoint = Arc<dyn Fn(u8) -> PathBuf + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectEvent {
    /// Unix milliseconds captured when the transport closes, for usage accounting.
    pub at: i64,
    /// Identifies the connection that closed. Ignore events from older connections.
    pub connection_id: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub opcode: u32,
    pub data: Value,
}

pub fn encode_frame(opcode: u32, data: &Value) -> Result<Vec<u8>, String> {
    let body = serde_json::to_vec(data).map_err(|error| error.to_string())?;
    if body.len() > MAX_FRAME_BYTES {
        return Err("Discord IPC request is too large.".into());
    }
    let mut bytes = Vec::with_capacity(8 + body.len());
    bytes.extend_from_slice(&opcode.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    Ok(bytes)
}

#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Frame>, String> {
        self.buffer.extend_from_slice(chunk);
        let mut consumed = 0;
        let mut frames = Vec::new();
        while self.buffer.len() - consumed >= 8 {
            let header = &self.buffer[consumed..consumed + 8];
            let opcode = u32::from_le_bytes(header[..4].try_into().unwrap());
            let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
            if length > MAX_FRAME_BYTES {
                return Err("Discord sent an oversized IPC frame.".into());
            }
            if self.buffer.len() - consumed < 8 + length {
                break;
            }
            let data = serde_json::from_slice(&self.buffer[consumed + 8..consumed + 8 + length])
                .map_err(|error| format!("Invalid Discord response: {error}"))?;
            frames.push(Frame { opcode, data });
            consumed += 8 + length;
        }
        if consumed != 0 {
            self.buffer.drain(..consumed);
        }
        Ok(frames)
    }
}

struct Request {
    nonce: String,
    activity: Value,
    response: oneshot::Sender<Result<(), String>>,
}

/// Shared observations and notifications for one transport generation.
struct ConnectionState {
    connected: Arc<AtomicBool>,
    disconnected_at: Arc<AtomicI64>,
    disconnects: broadcast::Sender<DisconnectEvent>,
    connection_id: u64,
}

/// A background reader keeps processing ping/pong and disconnects even while idle.
pub struct DiscordRpc {
    client_id: String,
    connection_id: u64,
    connected: Arc<AtomicBool>,
    disconnected_at: Arc<AtomicI64>,
    requests: Option<mpsc::Sender<Request>>,
    task: Option<JoinHandle<()>>,
    disconnects: broadcast::Sender<DisconnectEvent>,
    endpoint: Option<Endpoint>,
    response_timeout: Duration,
}

impl Default for DiscordRpc {
    fn default() -> Self {
        Self::new()
    }
}

impl DiscordRpc {
    pub fn new() -> Self {
        let (disconnects, _) = broadcast::channel(32);
        Self {
            client_id: String::new(),
            connection_id: 0,
            connected: Arc::new(AtomicBool::new(false)),
            disconnected_at: Arc::new(AtomicI64::new(0)),
            requests: None,
            task: None,
            disconnects,
            endpoint: None,
            response_timeout: RESPONSE_TIMEOUT,
        }
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn connection_id(&self) -> u64 {
        self.connection_id
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn disconnected_at(&self) -> Option<i64> {
        match self.disconnected_at.load(Ordering::Acquire) {
            0 => None,
            at => Some(at),
        }
    }

    pub fn subscribe_disconnects(&self) -> broadcast::Receiver<DisconnectEvent> {
        self.disconnects.subscribe()
    }

    pub async fn connect(&mut self, client_id: &str) -> Result<String, String> {
        self.disconnect();
        for index in 0..10 {
            let location = self
                .endpoint
                .as_ref()
                .map(|endpoint| endpoint(index))
                .unwrap_or_else(|| default_endpoint(index));
            let stream = match open_transport(&location).await {
                Ok(stream) => stream,
                Err(error) if endpoint_unavailable(&error) => continue,
                Err(error) => return Err(format!("Could not connect to Discord: {error}")),
            };
            let (requests, receiver) = mpsc::channel(32);
            let (ready, response) = oneshot::channel();
            // A previous task may still be winding down after abort. Give each
            // connection its own flag so it cannot clear a replacement's state.
            self.connected = Arc::new(AtomicBool::new(false));
            self.disconnected_at = Arc::new(AtomicI64::new(0));
            self.requests = Some(requests);
            self.task = Some(tokio::spawn(run_connection(
                stream,
                client_id.to_owned(),
                receiver,
                ready,
                ConnectionState {
                    connected: Arc::clone(&self.connected),
                    disconnected_at: Arc::clone(&self.disconnected_at),
                    disconnects: self.disconnects.clone(),
                    connection_id: self.connection_id,
                },
            )));
            let result = match tokio::time::timeout(self.response_timeout, response).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => {
                    Err("Discord closed the connection. Check your application ID.".into())
                }
                Err(_) => Err(HANDSHAKE_TIMEOUT.into()),
            };
            match result {
                Ok(user) => {
                    self.client_id = client_id.to_owned();
                    return Ok(user);
                }
                Err(error) => {
                    self.disconnect();
                    return Err(error);
                }
            }
        }
        Err("Discord desktop was not found. Open Discord and try again.".into())
    }

    /// `Value::Null` clears the activity, and still waits for Discord's ACK.
    pub async fn set_activity(&mut self, activity: Value) -> Result<(), String> {
        if !self.is_connected() {
            return Err(NOT_CONNECTED.into());
        }
        let requests = self.requests.as_ref().ok_or(NOT_CONNECTED)?;
        let (response, receiver) = oneshot::channel();
        let request = Request {
            nonce: Uuid::new_v4().to_string(),
            activity,
            response,
        };
        // Bound queuing and acknowledgment together, including an unresponsive pipe.
        match tokio::time::timeout(self.response_timeout, async {
            requests
                .send(request)
                .await
                .map_err(|_| DISCONNECTED.to_string())?;
            receiver
                .await
                .map_err(|_| "Connection closed.".to_string())?
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err("Discord did not confirm the presence update.".into()),
        }
    }

    pub fn disconnect(&mut self) {
        self.connected.store(false, Ordering::Release);
        // Replace the timestamp slot so a winding-down task cannot restore an
        // obsolete timestamp after an intentional disconnect or reconnect.
        self.disconnected_at = Arc::new(AtomicI64::new(0));
        self.connection_id = self.connection_id.wrapping_add(1);
        self.client_id.clear();
        self.requests = None;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for DiscordRpc {
    fn drop(&mut self) {
        self.disconnect();
    }
}

async fn run_connection(
    mut stream: IpcStream,
    client_id: String,
    mut requests: mpsc::Receiver<Request>,
    ready: oneshot::Sender<Result<String, String>>,
    state: ConnectionState,
) {
    let ConnectionState {
        connected,
        disconnected_at,
        disconnects,
        connection_id,
    } = state;
    let mut ready = Some(ready);
    let mut pending: HashMap<String, oneshot::Sender<Result<(), String>>> = HashMap::new();
    let outcome: Result<(), String> = async {
        write_frame(&mut stream, 0, &json!({ "v": 1, "client_id": client_id })).await?;
        let mut decoder = FrameDecoder::new();
        let mut chunk = [0; 8192];
        let mut cleanup = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                read = stream.read(&mut chunk) => {
                    let count = read.map_err(|error| format!("Discord IPC read failed: {error}"))?;
                    if count == 0 {
                        return Err(if ready.is_some() {
                            "Discord closed the connection. Check your application ID."
                        } else { DISCONNECTED }.into());
                    }
                    for Frame { opcode, data } in decoder.push(&chunk[..count])? {
                        if opcode == 3 {
                            write_frame(&mut stream, 4, &data).await?;
                            continue;
                        }
                        if opcode == 2 {
                            return Err(message(&data, "message", "Discord rejected the connection."));
                        }
                        if data["evt"] == "READY" {
                            if let Some(response) = ready.take() {
                                let user = data["data"]["user"]["global_name"].as_str()
                                    .filter(|value| !value.is_empty())
                                    .or_else(|| data["data"]["user"]["username"].as_str().filter(|value| !value.is_empty()))
                                    .unwrap_or("Discord user");
                                connected.store(true, Ordering::Release);
                                let _ = response.send(Ok(user.to_owned()));
                            }
                        } else {
                            let response = data["nonce"].as_str().and_then(|nonce| pending.remove(nonce));
                            if let Some(response) = response {
                                let result = if data["evt"] == "ERROR" {
                                    Err(message(&data["data"], "message", "Discord rejected this activity."))
                                } else { Ok(()) };
                                let _ = response.send(result);
                            } else if data["evt"] == "ERROR" && ready.is_some() {
                                return Err(message(&data["data"], "message", "Discord rejected the application ID."));
                            }
                        }
                    }
                }
                request = requests.recv() => {
                    let Some(request) = request else { return Ok(()); };
                    if request.response.is_closed() { continue; }
                    let payload = json!({
                        "cmd": "SET_ACTIVITY",
                        "args": { "pid": std::process::id(), "activity": request.activity },
                        "nonce": request.nonce,
                    });
                    pending.insert(request.nonce, request.response);
                    write_frame(&mut stream, 1, &payload).await?;
                }
                _ = cleanup.tick() => pending.retain(|_, response| !response.is_closed()),
            }
        }
    }.await;
    let error = outcome.err().unwrap_or_else(|| "Connection closed.".into());
    // Publish the precise stop time before exposing disconnected state or waking
    // an awaiting request. Service operations can then clamp usage even when a
    // broadcast listener is still waiting for the service mutex.
    let disconnect_event = if connected.load(Ordering::Acquire) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        disconnected_at.store(at, Ordering::Release);
        connected
            .swap(false, Ordering::AcqRel)
            .then_some(DisconnectEvent {
                at,
                connection_id,
                message: DISCONNECTED.into(),
            })
    } else {
        None
    };
    if let Some(response) = ready.take() {
        let _ = response.send(Err(error.clone()));
    }
    for (_, response) in pending {
        let _ = response.send(Err(error.clone()));
    }
    if let Some(event) = disconnect_event {
        let _ = disconnects.send(event);
    }
}

fn message(data: &Value, field: &str, fallback: &str) -> String {
    data[field]
        .as_str()
        .filter(|message| !message.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

async fn write_frame(stream: &mut IpcStream, opcode: u32, data: &Value) -> Result<(), String> {
    stream
        .write_all(&encode_frame(opcode, data)?)
        .await
        .map_err(|error| format!("Discord IPC write failed: {error}"))
}

fn endpoint_unavailable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::PermissionDenied
    ) || cfg!(windows) && matches!(error.raw_os_error(), Some(2 | 3 | 5 | 231))
}

#[cfg(windows)]
fn default_endpoint(index: u8) -> PathBuf {
    PathBuf::from(format!(r"\\?\pipe\discord-ipc-{index}"))
}

#[cfg(not(windows))]
fn default_endpoint(index: u8) -> PathBuf {
    ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("discord-ipc-{index}"))
}

#[cfg(windows)]
async fn open_transport(location: &std::path::Path) -> io::Result<IpcStream> {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(location)
        .map(|pipe| Box::new(pipe) as IpcStream)
}

#[cfg(unix)]
async fn open_transport(location: &std::path::Path) -> io::Result<IpcStream> {
    tokio::net::UnixStream::connect(location)
        .await
        .map(|stream| Box::new(stream) as IpcStream)
}

#[cfg(not(any(unix, windows)))]
async fn open_transport(_: &std::path::Path) -> io::Result<IpcStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Discord IPC is unsupported on this platform.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    #[test]
    fn decoder_handles_fragmented_utf8_and_coalesced_frames() {
        let mut decoder = FrameDecoder::new();
        let first = encode_frame(1, &json!({ "title": "🎵 日本語" })).unwrap();
        let second = encode_frame(3, &json!({ "ping": true })).unwrap();
        assert!(decoder.push(&first[..5]).unwrap().is_empty());
        assert!(decoder.push(&first[5..first.len() - 2]).unwrap().is_empty());
        let remainder = [first[first.len() - 2..].to_vec(), second].concat();
        assert_eq!(
            decoder.push(&remainder).unwrap(),
            vec![
                Frame {
                    opcode: 1,
                    data: json!({ "title": "🎵 日本語" })
                },
                Frame {
                    opcode: 3,
                    data: json!({ "ping": true })
                },
            ]
        );
    }

    #[test]
    fn decoder_rejects_oversized_header_and_invalid_json() {
        let mut header = [0u8; 8];
        header[4..].copy_from_slice(&(2u32 * 1024 * 1024).to_le_bytes());
        assert!(FrameDecoder::new()
            .push(&header)
            .unwrap_err()
            .contains("oversized"));
        assert!(FrameDecoder::new()
            .push(&[1, 0, 0, 0, 1, 0, 0, 0, b'{'])
            .is_err());
    }

    #[cfg(windows)]
    fn mock_connection() -> (PathBuf, impl Future<Output = IpcStream> + Send) {
        let path = PathBuf::from(format!(r"\\?\pipe\presence-rust-test-{}", Uuid::new_v4()));
        let pipe = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .create(&path)
            .unwrap();
        (path, async move {
            pipe.connect().await.unwrap();
            Box::new(pipe) as IpcStream
        })
    }

    #[cfg(unix)]
    fn mock_connection() -> (PathBuf, impl Future<Output = IpcStream> + Send) {
        let path = std::env::temp_dir().join(format!("presence-rust-{}", Uuid::new_v4()));
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let cleanup_path = path.clone();
        (path, async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(listener);
            let _ = std::fs::remove_file(cleanup_path);
            Box::new(stream) as IpcStream
        })
    }

    async fn read_frame(stream: &mut IpcStream) -> Frame {
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).await.unwrap();
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        assert!(length <= MAX_FRAME_BYTES);
        let mut body = vec![0u8; length];
        stream.read_exact(&mut body).await.unwrap();
        Frame {
            opcode: u32::from_le_bytes(header[..4].try_into().unwrap()),
            data: serde_json::from_slice(&body).unwrap(),
        }
    }

    fn test_rpc(path: PathBuf) -> DiscordRpc {
        let mut rpc = DiscordRpc::new();
        rpc.endpoint = Some(Arc::new(move |_| path.clone()));
        rpc.response_timeout = Duration::from_secs(2);
        rpc
    }

    #[tokio::test]
    async fn handshake_ping_ack_rejection_and_clear_use_real_ipc() {
        let (path, accepted) = mock_connection();
        let server = tokio::spawn(async move {
            let mut stream = accepted.await;
            let handshake = read_frame(&mut stream).await;
            assert_eq!(handshake.opcode, 0);
            assert_eq!(
                handshake.data,
                json!({ "v": 1, "client_id": "123456789012345678" })
            );
            let ready = encode_frame(1, &json!({ "evt": "READY", "data": { "user": { "global_name": "Test User", "username": "fallback" } } })).unwrap();
            stream.write_all(&ready[..6]).await.unwrap();
            stream
                .write_all(
                    &[
                        ready[6..].to_vec(),
                        encode_frame(3, &json!({ "nonce": "ping" })).unwrap(),
                    ]
                    .concat(),
                )
                .await
                .unwrap();
            let mut pong = false;
            let mut requests = 0;
            while requests < 3 || !pong {
                let frame = read_frame(&mut stream).await;
                if frame.opcode == 4 {
                    assert_eq!(frame.data, json!({ "nonce": "ping" }));
                    pong = true;
                    continue;
                }
                assert_eq!(frame.opcode, 1);
                assert_eq!(frame.data["cmd"], "SET_ACTIVITY");
                assert_eq!(frame.data["args"]["pid"], std::process::id());
                let activity = &frame.data["args"]["activity"];
                if requests == 2 {
                    assert!(activity.is_null());
                }
                let reject = activity["details"] == "reject";
                write_frame(&mut stream, 1, &json!({
                    "nonce": frame.data["nonce"],
                    "evt": if reject { json!("ERROR") } else { Value::Null },
                    "data": if reject { json!({ "message": "Invalid activity" }) } else { activity.clone() },
                })).await.unwrap();
                requests += 1;
            }
            // Keep the transport alive until the client deliberately disconnects.
            let mut byte = [0];
            let _ = stream.read(&mut byte).await;
        });
        let mut rpc = test_rpc(path);
        let mut disconnected = rpc.subscribe_disconnects();
        assert_eq!(
            rpc.connect("123456789012345678").await.unwrap(),
            "Test User"
        );
        assert!(rpc.is_connected());
        rpc.set_activity(json!({ "details": "Hello", "type": 0 }))
            .await
            .unwrap();
        assert_eq!(
            rpc.set_activity(json!({ "details": "reject" }))
                .await
                .unwrap_err(),
            "Invalid activity"
        );
        rpc.set_activity(Value::Null).await.unwrap();
        rpc.disconnect();
        server.await.unwrap();
        assert!(!rpc.is_connected());
        assert!(rpc.client_id().is_empty());
        assert!(matches!(
            disconnected.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn handshake_rejections_are_returned_to_the_caller() {
        for (opcode, payload) in [
            (2, json!({ "message": "Bad application ID" })),
            (
                1,
                json!({ "evt": "ERROR", "nonce": "handshake", "data": { "message": "Bad application ID" } }),
            ),
        ] {
            let (path, accepted) = mock_connection();
            let server = tokio::spawn(async move {
                let mut stream = accepted.await;
                read_frame(&mut stream).await;
                write_frame(&mut stream, opcode, &payload).await.unwrap();
            });
            let mut rpc = test_rpc(path);
            assert_eq!(rpc.connect("bad").await.unwrap_err(), "Bad application ID");
            assert!(!rpc.is_connected());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn missing_ready_and_missing_ack_time_out() {
        for acknowledge_handshake in [false, true] {
            let (path, accepted) = mock_connection();
            let (release, released) = oneshot::channel::<()>();
            let server = tokio::spawn(async move {
                let mut stream = accepted.await;
                read_frame(&mut stream).await;
                if acknowledge_handshake {
                    write_frame(&mut stream, 1, &json!({ "evt": "READY" }))
                        .await
                        .unwrap();
                    read_frame(&mut stream).await;
                }
                let _ = released.await;
            });
            let mut rpc = test_rpc(path);
            rpc.response_timeout = Duration::from_millis(30);
            let error = if acknowledge_handshake {
                assert_eq!(rpc.connect("123").await.unwrap(), "Discord user");
                rpc.set_activity(Value::Null).await.unwrap_err()
            } else {
                rpc.connect("123").await.unwrap_err()
            };
            assert!(error.contains(if acknowledge_handshake {
                "did not confirm"
            } else {
                "did not respond"
            }));
            rpc.disconnect();
            let _ = release.send(());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn idle_disconnect_reports_connection_and_time() {
        let (path, accepted) = mock_connection();
        let (close, closed) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let mut stream = accepted.await;
            read_frame(&mut stream).await;
            write_frame(&mut stream, 1, &json!({ "evt": "READY" }))
                .await
                .unwrap();
            let _ = closed.await;
        });
        let mut rpc = test_rpc(path);
        let mut events = rpc.subscribe_disconnects();
        rpc.connect("123").await.unwrap();
        let id = rpc.connection_id();
        close.send(()).unwrap();
        let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.connection_id, id);
        assert_eq!(event.message, DISCONNECTED);
        assert!(event.at > 0);
        assert!(!rpc.is_connected());
        assert_eq!(rpc.disconnected_at(), Some(event.at));
        rpc.disconnect();
        assert_ne!(rpc.connection_id(), id);
        assert_eq!(rpc.disconnected_at(), None);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn transport_close_rejects_an_unacknowledged_request() {
        let (path, accepted) = mock_connection();
        let server = tokio::spawn(async move {
            let mut stream = accepted.await;
            read_frame(&mut stream).await;
            write_frame(&mut stream, 1, &json!({ "evt": "READY" }))
                .await
                .unwrap();
            read_frame(&mut stream).await;
            // Drop the pipe with a nonce still pending.
        });
        let mut rpc = test_rpc(path);
        rpc.connect("123").await.unwrap();
        let error = rpc
            .set_activity(json!({ "details": "Pending" }))
            .await
            .unwrap_err();
        assert!(error.contains("disconnected") || error.contains("read failed"));
        assert!(!rpc.is_connected());
        server.await.unwrap();
    }
}
