//! Test-binary-only handshake at durable bootstrap boundaries; absent from release builds.

/// Record a durable boundary and pause there only when the parent test requests it.
pub fn reach(name: &str) {
    use std::io::Write;
    let Ok(root) = std::env::var("USDB_TEST_BOOTSTRAP_PROCESS_ROOT") else {
        return;
    };
    let root = std::path::Path::new(&root);
    let mut events = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("boundaries.log"))
        .unwrap();
    writeln!(events, "{} {name}", std::process::id()).unwrap();
    if std::env::var("USDB_TEST_BOOTSTRAP_PAUSE").as_deref() != Ok(name) {
        return;
    }
    let marker = root.join("reached.json");
    let pending = root.join("reached.tmp");
    std::fs::write(
        &pending,
        serde_json::to_vec(&serde_json::json!({"phase":name,"pid":std::process::id()})).unwrap(),
    )
    .unwrap();
    std::fs::rename(pending, marker).unwrap();
    // The parent observes the marker and kills this process. No fixed sleep guesses a phase.
    loop {
        std::thread::park();
    }
}
