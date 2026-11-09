//! A GET whose connection dies before the answer is asked again (iOS kills
//! the sockets of a suspended app: every download in flight failed with
//! "Bad file descriptor" and stayed failed); an HTTP answer, even an error
//! status, is never asked twice.
#![cfg(not(target_family = "wasm"))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A server that drops the first `drops` connections after reading their
/// request, then answers `status` with "ok"; returns its URL and the count
/// of connections it took.
fn server(drops: usize, status: u16) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/blob", listener.local_addr().unwrap());
    let seen = Arc::new(AtomicUsize::new(0));
    let count = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let n = count.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            if n < drops {
                drop(stream);
                continue;
            }
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    (url, seen)
}

fn get(url: &str) -> anyhow::Result<roostr_core::http::Response> {
    futures::executor::block_on(roostr_core::http::get(url, &[], None, 1 << 20))
}

#[test]
fn a_dropped_connection_is_asked_again() {
    let (url, seen) = server(1, 200);
    let res = get(&url).expect("the second try answers");
    assert_eq!((res.status, res.body.as_slice()), (200, b"ok".as_slice()));
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

#[test]
fn a_server_that_never_answers_is_asked_a_bounded_number_of_times() {
    let (url, seen) = server(usize::MAX, 200);
    assert!(get(&url).is_err());
    assert_eq!(seen.load(Ordering::SeqCst), 3);
}

#[test]
fn an_error_status_is_an_answer_not_retried() {
    let (url, seen) = server(0, 404);
    assert_eq!(get(&url).unwrap().status, 404);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}
