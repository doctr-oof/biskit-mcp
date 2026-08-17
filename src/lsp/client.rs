use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

/// Every critical section here is one non-async insert or remove, so a synchronous mutex fits
/// better than an async one: no task state machine, no yield point, no lock held across an await.
type PendingMap =
    Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Result<Value, ResponseError>>>>>;

/// Reported to a waiting request when the read loop sees the server go away.
pub const TERMINATED_CODE: i64 = -32000;

#[derive(Debug, Clone)]
pub struct ResponseError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "language server error {}: {}",
            self.code, self.message
        )
    }
}

impl std::error::Error for ResponseError {}

/// The language server stopped answering at all, as opposed to failing one request.
///
/// The distinction matters to any caller that issues a request per file: one file's parse failure
/// is worth skipping past, whereas a server that has stopped answering will burn a full timeout on
/// every remaining file for no possible result.
#[derive(Debug, Clone)]
pub struct Unavailable {
    pub method: String,
    pub detail: String,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "language server did not answer {}: {}",
            self.method, self.detail
        )
    }
}

impl std::error::Error for Unavailable {}

/// True when `error` means the server is gone rather than that one request failed.
pub fn is_unavailable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Unavailable>().is_some()
        || error
            .downcast_ref::<ResponseError>()
            .is_some_and(|response| response.code == TERMINATED_CODE)
}

fn unavailable(method: &str, detail: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Unavailable {
        method: method.to_string(),
        detail: detail.into(),
    })
}

pub enum ServerEvent {
    LogMessage(String),
    Exited,
}

/// A raw JSON-RPC connection to a language server subprocess over stdio.
pub struct LspConnection {
    child: Mutex<Option<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingMap,
    next_id: AtomicI64,
    reader: JoinHandle<()>,
    stderr_reader: JoinHandle<()>,
    request_timeout: Duration,
}

impl LspConnection {
    pub async fn spawn(
        program: &std::path::Path,
        args: &[String],
        working_directory: &std::path::Path,
        request_timeout: Duration,
        events: mpsc::UnboundedSender<ServerEvent>,
        configuration: Value,
    ) -> Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start language server: {}", program.display()))?;

        let stdin = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .ok_or_else(|| anyhow!("language server stdin unavailable"))?,
        ));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("language server stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("language server stderr unavailable"))?;

        let pending: PendingMap = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let reader = tokio::spawn(read_loop(
            BufReader::new(stdout),
            Arc::clone(&pending),
            Arc::clone(&stdin),
            events.clone(),
            configuration,
        ));
        let stderr_reader = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "biskit::lsp", "{line}");
            }
        });

        Ok(Self {
            child: Mutex::new(Some(child)),
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            reader,
            stderr_reader,
            request_timeout,
        })
    }

    pub async fn request<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let value = self
            .request_value(method, params, self.request_timeout)
            .await?;
        serde_json::from_value(value)
            .with_context(|| format!("unexpected response shape for {method}"))
    }

    pub async fn request_with_timeout<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Value,
        duration: Duration,
    ) -> Result<T> {
        let value = self.request_value(method, params, duration).await?;
        serde_json::from_value(value)
            .with_context(|| format!("unexpected response shape for {method}"))
    }

    async fn request_value(
        &self,
        method: &str,
        params: Value,
        duration: Duration,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        pending_insert(&self.pending, id, sender);

        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        if let Err(error) = write_message(&self.stdin, &message).await {
            pending_remove(&self.pending, id);
            return Err(error);
        }

        match timeout(duration, receiver).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(error.into()),
            Ok(Err(_)) => Err(unavailable(method, "the connection closed")),
            Err(_) => {
                pending_remove(&self.pending, id);
                Err(unavailable(
                    method,
                    format!("no response within {}ms", duration.as_millis()),
                ))
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let message = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_message(&self.stdin, &message).await
    }

    pub async fn shutdown(&self) {
        let graceful = async {
            let _: Value = self
                .request_with_timeout("shutdown", Value::Null, Duration::from_secs(3))
                .await?;
            self.notify("exit", Value::Null).await
        };
        let _ = timeout(Duration::from_secs(5), graceful).await;

        self.reader.abort();
        self.stderr_reader.abort();

        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(3), child.wait()).await;
        }
    }
}

fn pending_insert(
    pending: &PendingMap,
    id: i64,
    sender: oneshot::Sender<Result<Value, ResponseError>>,
) {
    if let Ok(mut guard) = pending.lock() {
        guard.insert(id, sender);
    }
}

fn pending_remove(
    pending: &PendingMap,
    id: i64,
) -> Option<oneshot::Sender<Result<Value, ResponseError>>> {
    pending.lock().ok()?.remove(&id)
}

/// `ChildStdin` is an unbuffered pipe, so the header and the body go out as one write rather than
/// as two syscalls plus a flush.
async fn write_message(stdin: &Arc<Mutex<ChildStdin>>, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    let mut framed = Vec::with_capacity(body.len() + 32);
    framed.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
    framed.extend_from_slice(&body);

    let mut guard = stdin.lock().await;
    guard.write_all(&framed).await?;
    guard.flush().await?;
    Ok(())
}

async fn read_loop(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    pending: PendingMap,
    stdin: Arc<Mutex<ChildStdin>>,
    events: mpsc::UnboundedSender<ServerEvent>,
    configuration: Value,
) {
    // Both buffers are reused for the life of the connection: a `documentSymbol` response is
    // large, and allocating and zero-filling a fresh buffer for each one is pure overhead.
    let mut header = String::new();
    let mut body: Vec<u8> = Vec::new();

    loop {
        let Ok(Some(message)) = read_message(&mut stdout, &mut header, &mut body).await else {
            let _ = events.send(ServerEvent::Exited);
            if let Ok(mut guard) = pending.lock() {
                for (_, sender) in guard.drain() {
                    let _ = sender.send(Err(ResponseError {
                        code: TERMINATED_CODE,
                        message: "language server terminated".to_string(),
                    }));
                }
            }
            return;
        };

        let has_id = message.get("id").is_some_and(|id| !id.is_null());
        let method = message.get("method").and_then(Value::as_str);

        match (method, has_id) {
            (Some(method), true) => {
                let id = message["id"].clone();
                let result = respond_to_server_request(method, &message, &configuration);
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                let _ = write_message(&stdin, &reply).await;
            }
            (Some(method), false) => handle_notification(method, &message, &events),
            (None, true) => {
                let Some(id) = message["id"].as_i64() else {
                    continue;
                };
                let outcome = match message.get("error") {
                    Some(error) => Err(ResponseError {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string(),
                    }),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if let Some(sender) = pending_remove(&pending, id) {
                    let _ = sender.send(outcome);
                }
            }
            (None, false) => {}
        }
    }
}

fn respond_to_server_request(method: &str, message: &Value, configuration: &Value) -> Value {
    match method {
        "workspace/configuration" => {
            let count = message
                .get("params")
                .and_then(|params| params.get("items"))
                .and_then(Value::as_array)
                .map_or(1, Vec::len);
            Value::Array(vec![configuration.clone(); count])
        }
        "window/workDoneProgress/create"
        | "client/registerCapability"
        | "client/unregisterCapability" => Value::Null,
        _ => Value::Null,
    }
}

fn handle_notification(method: &str, message: &Value, events: &mpsc::UnboundedSender<ServerEvent>) {
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        // Diagnostics are pulled on demand via textDocument/diagnostic, so pushes are ignored.
        "window/logMessage" | "window/showMessage" => {
            if let Some(text) = params.get("message").and_then(Value::as_str) {
                let _ = events.send(ServerEvent::LogMessage(text.to_string()));
            }
        }
        _ => {}
    }
}

async fn read_message(
    stdout: &mut BufReader<tokio::process::ChildStdout>,
    header: &mut String,
    body: &mut Vec<u8>,
) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        header.clear();
        let read = stdout.read_line(header).await?;
        if read == 0 {
            return Ok(None);
        }
        let line = header.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = Some(value.trim().parse()?);
        }
    }

    let length =
        content_length.ok_or_else(|| anyhow!("language server message lacked Content-Length"))?;
    body.clear();
    body.resize(length, 0);
    stdout.read_exact(body).await?;
    Ok(Some(serde_json::from_slice(body)?))
}
