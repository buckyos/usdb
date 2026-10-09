use super::serve_until_shutdown;
use axum::{Router, routing::get};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};

/// Shutdown closes the listener while allowing an already accepted write to finish.
#[tokio::test]
async fn shutdown_drains_in_flight_request_before_exit() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let app = Router::new().route(
            "/write",
            get({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        "write completed"
                    }
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let server = tokio::spawn(serve_until_shutdown(listener, app, async {
            stop_rx.await.unwrap();
            "test"
        }));
        let request = tokio::spawn(async move {
            reqwest::get(format!("http://{address}/write"))
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        entered.notified().await;
        stop_tx.send(()).unwrap();
        // Observe the actual listener closing rather than assuming a scheduler delay.
        loop {
            if tokio::net::TcpStream::connect(address).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !server.is_finished(),
            "active request must finish before exit"
        );
        release.notify_one();
        assert_eq!(request.await.unwrap(), "write completed");
        server.await.unwrap().unwrap();
    })
    .await
    .expect("graceful shutdown did not complete");
}

/// An idle keep-alive connection must not hold shutdown open indefinitely.
#[tokio::test]
async fn shutdown_closes_idle_keep_alive_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let server = tokio::spawn(serve_until_shutdown(
            listener,
            Router::new().route("/healthz", get(|| async { "ok" })),
            async {
                stop_rx.await.unwrap();
                "test"
            },
        ));
        let mut connection = tokio::net::TcpStream::connect(address).await.unwrap();
        connection
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\nok") {
            response.push(connection.read_u8().await.unwrap());
        }
        stop_tx.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(connection.read(&mut [0_u8; 1]).await.unwrap(), 0);
    })
    .await
    .expect("idle connection prevented shutdown");
}
