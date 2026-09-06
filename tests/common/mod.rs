#![allow(dead_code)]

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Reply {
    pub status: u16,
    pub body: String,
    pub before_headers: Duration,
    pub before_body: Duration,
    pub on_request: Option<Box<dyn FnOnce() + Send>>,
    pub on_headers: Option<Box<dyn FnOnce() + Send>>,
}
impl Reply {
    pub fn ok(body: &str) -> Self {
        Self {
            status: 200,
            body: body.into(),
            before_headers: Duration::ZERO,
            before_body: Duration::ZERO,
            on_request: None,
            on_headers: None,
        }
    }
}

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    pub fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let recorded = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            while !stopped.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("accept: {e}"),
                };
                // Accepted sockets inherit O_NONBLOCK on macOS/BSD. Only the
                // listener should poll; request reads must wait for their bytes.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                loop {
                    let mut line = String::new();
                    let size = reader.read_line(&mut line).unwrap();
                    if size == 0 || line == "\r\n" {
                        break;
                    }
                    request.push_str(&line);
                }
                recorded.lock().unwrap().push(request);
                let reply = replies.pop_front().expect("unexpected HTTP request");
                if let Some(on_request) = reply.on_request {
                    on_request();
                }
                thread::sleep(reply.before_headers);
                let header = format!(
                    "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                    reply.status,
                    reply.body.len()
                );
                if stream.write_all(header.as_bytes()).is_ok() {
                    if let Some(on_headers) = reply.on_headers {
                        on_headers();
                    }
                    thread::sleep(reply.before_body);
                    // A timeout or cancellation may close the peer before the response.
                    let _ = stream.write_all(reply.body.as_bytes());
                }
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

pub fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for test condition"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
