use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

// ── Wire protocol types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Message {
        from_id: String,
        from_slug: String,
        message: String,
    },
    Query {
        from_id: String,
        question: String,
    },
    QueryResult {
        answer: String,
    },
    QueryError {
        error: String,
    },
}

// ── Incoming message (delivered to the engine) ──────────────────────────────

#[derive(Debug)]
pub enum IncomingMessage {
    Message {
        from_id: String,
        from_slug: String,
        message: String,
    },
    Query {
        from_id: String,
        question: String,
        reply_tx: tokio::sync::oneshot::Sender<String>,
    },
}

// ── Socket listener ─────────────────────────────────────────────────────────

/// Start listening on a Unix domain socket. Returns the socket path and a
/// receiver for incoming messages. The listener runs in a background tokio task.
pub fn start_listener(
    pid: u32,
) -> std::io::Result<(PathBuf, mpsc::UnboundedReceiver<IncomingMessage>)> {
    let dir = crate::paths::state_dir().join("sockets");
    std::fs::create_dir_all(&dir)?;

    let socket_path = dir.join(format!("{pid}.sock"));

    // Clean up stale socket from a previous run.
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path)?;
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(accept_loop(listener, tx));

    Ok((socket_path, rx))
}

async fn accept_loop(listener: UnixListener, tx: mpsc::UnboundedSender<IncomingMessage>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(_) => break,
        };
        let tx = tx.clone();
        tokio::spawn(handle_connection(stream, tx));
    }
}

async fn handle_connection(stream: UnixStream, tx: mpsc::UnboundedSender<IncomingMessage>) {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    if buf_reader.read_line(&mut line).await.is_err() {
        return;
    }

    let msg: WireMessage = match serde_json::from_str(line.trim()) {
        Ok(m) => m,
        Err(_) => return,
    };

    match msg {
        WireMessage::Message {
            from_id,
            from_slug,
            message,
        } => {
            let _ = tx.send(IncomingMessage::Message {
                from_id,
                from_slug,
                message,
            });
        }
        WireMessage::Query { from_id, question } => {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(IncomingMessage::Query {
                from_id,
                question,
                reply_tx,
            });

            // Wait for the answer (btw-style call runs in the background).
            let response =
                match tokio::time::timeout(std::time::Duration::from_secs(30), reply_rx).await {
                    Ok(Ok(answer)) => WireMessage::QueryResult { answer },
                    Ok(Err(_)) => WireMessage::QueryError {
                        error: "query handler dropped".into(),
                    },
                    Err(_) => WireMessage::QueryError {
                        error: "query timed out".into(),
                    },
                };

            if let Ok(json) = serde_json::to_string(&response) {
                let _ = writer.write_all(json.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                let _ = writer.flush().await;
            }
        }
        // Responses are only sent by the listener, never received.
        WireMessage::QueryResult { .. } | WireMessage::QueryError { .. } => {}
    }
}

// ── Socket client ───────────────────────────────────────────────────────────

/// Send a message to a target agent. Fire-and-forget.
/// Returns Ok(()) on success, Err(message) if unreachable.
pub async fn send_message(
    socket_path: &Path,
    from_id: &str,
    from_slug: &str,
    message: &str,
) -> Result<(), String> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(socket_path),
    )
    .await
    .map_err(|_| "connection timed out".to_string())?
    .map_err(|e| format!("connection failed: {e}"))?;

    let msg = WireMessage::Message {
        from_id: from_id.to_string(),
        from_slug: from_slug.to_string(),
        message: message.to_string(),
    };

    let mut buf = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    buf.push('\n');

    let (_, mut writer) = stream.into_split();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        writer.write_all(buf.as_bytes()),
    )
    .await
    .map_err(|_| "write timed out".to_string())?
    .map_err(|e| format!("write failed: {e}"))?;

    Ok(())
}

/// Send a query to a target agent and wait for the answer.
pub async fn send_query(
    socket_path: &Path,
    from_id: &str,
    question: &str,
) -> Result<String, String> {
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        UnixStream::connect(socket_path),
    )
    .await
    .map_err(|_| "connection timed out".to_string())?
    .map_err(|e| format!("connection failed: {e}"))?;

    let msg = WireMessage::Query {
        from_id: from_id.to_string(),
        question: question.to_string(),
    };

    let mut buf = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    buf.push('\n');

    let (reader, mut writer) = stream.into_split();

    // Send the query.
    writer
        .write_all(buf.as_bytes())
        .await
        .map_err(|e| format!("write failed: {e}"))?;

    // Wait for response with timeout.
    let mut buf_reader = BufReader::new(reader);
    let mut response_line = String::new();

    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        buf_reader.read_line(&mut response_line),
    )
    .await
    .map_err(|_| "query timed out (30s)".to_string())?
    .map_err(|e| format!("read failed: {e}"))?;

    let response: WireMessage =
        serde_json::from_str(response_line.trim()).map_err(|e| format!("parse error: {e}"))?;

    match response {
        WireMessage::QueryResult { answer } => Ok(answer),
        WireMessage::QueryError { error } => Err(error),
        _ => Err("unexpected response type".into()),
    }
}

/// Clean up the socket file for this PID.
pub fn cleanup_socket(pid: u32) {
    let path = crate::paths::state_dir()
        .join("sockets")
        .join(format!("{pid}.sock"));
    let _ = std::fs::remove_file(path);
}
