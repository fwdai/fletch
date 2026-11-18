//! A local HTTP server standing in for a cloud provider's API in tests. Every
//! request is recorded and answered by a function the test supplies, so a test
//! drives the exact answers a provider would give and then asserts the exact
//! requests it was sent — method, path with query, headers and JSON body.

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// One request the server received.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: String,
    /// The path with its query, as sent: `/v1/apps?org_slug=personal`.
    pub path: String,
    headers: Vec<(String, String)>,
    /// The JSON body; `Null` when there was none.
    pub body: Value,
}

impl Request {
    /// A header's value, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// `"POST /v1/apps"`: the method and the path with its query.
    pub fn line(&self) -> String {
        format!("{} {}", self.method, self.path)
    }
}

/// The server's answer to one request.
pub(crate) struct Reply {
    status: u16,
    body: Option<Value>,
    headers: Vec<(String, String)>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body: Some(body),
            headers: Vec::new(),
        }
    }

    /// An answer with no body.
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: None,
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn encode(&self) -> Vec<u8> {
        let body = self.body.as_ref().map(Value::to_string).unwrap_or_default();
        let reason = reqwest::StatusCode::from_u16(self.status)
            .ok()
            .and_then(|s| s.canonical_reason())
            .unwrap_or("Status");
        let mut head = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n",
            self.status,
            body.len()
        );
        for (name, value) in &self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }
}

/// A running stand-in. Lives as long as the test's runtime.
pub(crate) struct StandIn {
    url: String,
    log: Arc<Mutex<Vec<Request>>>,
}

impl StandIn {
    /// Serve on a free local port, answering every request with `answer`.
    pub async fn start(answer: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        let answer = Arc::new(answer);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let seen = seen.clone();
                let answer = answer.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut stream).await else {
                        return;
                    };
                    // Recorded before answering, so the log holds a request by
                    // the time its client sees the reply.
                    seen.lock().unwrap().push(request.clone());
                    let reply = answer(&request);
                    let _ = stream.write_all(&reply.encode()).await;
                });
            }
        });
        Self { url, log }
    }

    /// The base URL a client is pointed at.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn requests(&self) -> Vec<Request> {
        self.log.lock().unwrap().clone()
    }

    /// [`Request::line`] of every request, in order.
    pub fn lines(&self) -> Vec<String> {
        self.requests().iter().map(Request::line).collect()
    }
}

/// Read one request: its head, then as many body bytes as it declares.
async fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&buf[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = head_end + 4;
    while raw.len() < body_start + length {
        let n = stream.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }
    let body = serde_json::from_slice(&raw[body_start..]).unwrap_or(Value::Null);
    Some(Request {
        method,
        path,
        headers,
        body,
    })
}
