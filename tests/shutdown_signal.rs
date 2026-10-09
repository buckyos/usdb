use super::shutdown_signal;
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Run signal delivery in an isolated process, never against the parallel test runner.
#[test]
fn signals_received_before_polling_are_retained() {
    for signal in ["SIGTERM", "SIGINT"] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shutdown::tests::signal_child", "--nocapture"])
            .env("USDB_SHUTDOWN_TEST_SIGNAL", signal)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "child did not register handlers"
            );
            if line.contains("handlers registered") {
                break;
            }
        }
        let sent = Command::new("kill")
            .args(["-s", signal, &child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        // The child cannot poll the future before this explicit barrier is released.
        writeln!(child.stdin.take().unwrap(), "continue").unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{signal}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

/// Subprocess entry point for the early-startup signal regression.
#[test]
fn signal_child() {
    let Ok(expected) = std::env::var("USDB_SHUTDOWN_TEST_SIGNAL") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let shutdown = shutdown_signal("test startup").unwrap();
    println!("handlers registered");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_line(&mut String::new()).unwrap();
    let received = runtime
        .block_on(tokio::time::timeout(Duration::from_secs(5), shutdown))
        .expect("startup signal was lost before polling");
    assert_eq!(received, expected);
}
