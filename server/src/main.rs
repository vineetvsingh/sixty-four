//! Serves the built site from `dist/` and hosts games between two browsers.
//! Browsers refuse to load WebAssembly from file:// URLs, so the site needs an
//! HTTP server; this one uses only the standard library and the chess engine.
//!
//! Usage: serve [port] [dir]     (defaults: 8080, dist)
//! Environment: PORT sets the port, HOST the address to listen on (default
//! 127.0.0.1; use 0.0.0.0 to accept connections from other machines).

mod games;

use games::{Games, Refusal};
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Each open connection holds a thread, so there is a ceiling.
const MAX_CONNECTIONS: usize = 1024;
/// How long a browser's "tell me when something changes" request is held open.
const LONG_POLL: Duration = Duration::from_secs(25);

struct Shared {
    games: Mutex<Games>,
    /// Signalled whenever any game changes, to wake the requests waiting on it.
    changed: Condvar,
    connections: AtomicUsize,
}

impl Shared {
    fn games(&self) -> MutexGuard<'_, Games> {
        self.games.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Numbers nobody can guess, for game ids and seat secrets. The standard library has
/// no random generator of its own, but every `RandomState` is keyed from the system's.
fn random_u64() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "wav" => "audio/wav",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn respond(stream: &mut TcpStream, status: &str, ctype: &str, body: &[u8], head_only: bool) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    if !head_only {
        let _ = stream.write_all(body);
    }
}

/// Map a request target to a file under `root`, refusing anything that climbs out of it.
fn resolve(root: &Path, target: &str) -> Option<PathBuf> {
    let path = target.split(['?', '#']).next().unwrap_or("");
    let mut rel = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part.contains(['\\', ':', '%']) {
            return None;
        }
        rel.push(part);
    }
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let mut full = root.join(rel);
    if full.is_dir() {
        full.push("index.html");
    }
    Some(full)
}

/// Decode `a=1&b=two%20words` into its fields.
fn parse_form(s: &str) -> HashMap<String, String> {
    fn decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
            match b[i] {
                b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                    out.push(hex(b[i + 1]).unwrap_or(0) << 4 | hex(b[i + 2]).unwrap_or(0));
                    i += 2;
                }
                b'+' => out.push(b' '),
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    s.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (decode(k), decode(v)))
        .collect()
}

fn json_error(why: &str) -> String {
    format!("{{\"error\":\"{}\"}}", why.replace(['"', '\\'], ""))
}

/// Hold the request until the game is newer than version `seen`, or for 25 seconds.
fn wait_for_state(shared: &Shared, id: &str, seat: &str, seen: u64) -> Result<String, Refusal> {
    let give_up = Instant::now() + LONG_POLL;
    let mut games = shared.games();
    loop {
        let now = Instant::now();
        let (version, json) = games.state(id, seat, now)?;
        if version > seen || now >= give_up {
            return Ok(json);
        }
        // Wake up in time to call a flag fall, even if neither player does anything.
        let mut wait = give_up - now;
        if let Some(flag) = games.deadline(id, now) {
            wait = wait.min(flag.saturating_duration_since(now) + Duration::from_millis(20));
        }
        games = shared.changed.wait_timeout(games, wait).unwrap_or_else(|e| e.into_inner()).0;
    }
}

fn api(shared: &Shared, method: &str, path: &str, form: &HashMap<String, String>) -> Result<String, Refusal> {
    let get = |k: &str| form.get(k).map(String::as_str).unwrap_or("");
    let number = |k: &str| get(k).parse::<f64>().ok().filter(|n| n.is_finite()).unwrap_or(0.0);
    let now = Instant::now();
    let reply = match (method, path) {
        ("GET", "/api/state") => return wait_for_state(shared, get("id"), get("seat"), get("v").parse().unwrap_or(0)),
        ("POST", "/api/new") => shared.games().create(get("color"), number("base"), number("inc"), now),
        ("POST", "/api/join") => shared.games().join(get("id"), now),
        ("POST", "/api/move") => {
            let ply = get("ply").parse().map_err(|_| (400, "That request is incomplete."))?;
            shared.games().play(get("id"), get("seat"), get("san"), ply, now)
        }
        ("POST", "/api/act") => shared.games().act(get("id"), get("seat"), get("what"), now),
        _ => Err((404, "No such address.")),
    };
    shared.changed.notify_all();
    reply
}

fn handle(mut stream: TcpStream, root: &Path, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut line = String::new();
    let mut body = Vec::new();
    {
        let mut reader = BufReader::new((&stream).take(16 * 1024));
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let mut length = 0usize;
        let mut h = String::new();
        while reader.read_line(&mut h).map(|n| n > 2).unwrap_or(false) {
            if let Some((name, value)) = h.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            h.clear();
        }
        // Nothing the page sends is anywhere near this long.
        if length > 4096 {
            return;
        }
        body.resize(length, 0);
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let head_only = method == "HEAD";
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    if path.starts_with("/api/") {
        let form = parse_form(if method == "GET" { query } else { std::str::from_utf8(&body).unwrap_or("") });
        let (status, json) = match api(shared, method, path, &form) {
            Ok(json) => ("200 OK", json),
            Err((400, why)) => ("400 Bad Request", json_error(why)),
            Err((403, why)) => ("403 Forbidden", json_error(why)),
            Err((404, why)) => ("404 Not Found", json_error(why)),
            Err((409, why)) => ("409 Conflict", json_error(why)),
            Err((_, why)) => ("503 Service Unavailable", json_error(why)),
        };
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
            json.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(json.as_bytes());
        return;
    }
    if method != "GET" && !head_only {
        respond(&mut stream, "405 Method Not Allowed", "text/plain", b"Method not allowed", false);
        return;
    }
    match resolve(root, target).and_then(|p| fs::read(&p).ok().map(|b| (p, b))) {
        Some((p, body)) => respond(&mut stream, "200 OK", content_type(&p), &body, head_only),
        None => respond(&mut stream, "404 Not Found", "text/plain", b"Not found", head_only),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let env_port = std::env::var("PORT").ok().and_then(|p| p.parse().ok());
    let port: u16 = args.next().and_then(|p| p.parse().ok()).or(env_port).unwrap_or(8080);
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let root = PathBuf::from(args.next().unwrap_or_else(|| "dist".into()));
    if !root.join("index.html").is_file() {
        eprintln!("No index.html in '{}'. Build the site first (see README.md).", root.display());
        std::process::exit(1);
    }
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Cannot listen on {host}:{port}: {e}");
            std::process::exit(1);
        }
    };
    let shown = if host == "127.0.0.1" { "localhost" } else { host.as_str() };
    println!("Sixty-Four Chess is at http://{shown}:{port}  (Ctrl+C to stop)");
    let shared = Arc::new(Shared {
        games: Mutex::new(Games::new(Box::new(random_u64))),
        changed: Condvar::new(),
        connections: AtomicUsize::new(0),
    });
    for mut stream in listener.incoming().flatten() {
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            respond(&mut stream, "503 Service Unavailable", "text/plain", b"Busy", false);
            continue;
        }
        let (root, shared) = (root.clone(), shared.clone());
        thread::spawn(move || {
            handle(stream, &root, &shared);
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
}
