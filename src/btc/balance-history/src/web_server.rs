use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};

pub fn serve_static_files(port: u16, web_root: &Path) -> Result<(), String> {
    if !web_root.exists() {
        return Err(format!("Web root does not exist: {}", web_root.display()));
    }
    if !web_root.is_dir() {
        return Err(format!(
            "Web root is not a directory: {}",
            web_root.display()
        ));
    }

    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("Failed to bind web server on 127.0.0.1:{}: {}", port, e))?;

    info!(
        "Web server started: module=web, listen_addr=127.0.0.1:{}, web_root={}",
        port,
        web_root.display()
    );
    println!("Web server started at http://127.0.0.1:{}", port);
    println!("Serving static files from {}", web_root.display());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(e) = handle_connection(stream, web_root) {
                    warn!("Web request failed: module=web, error={}", e);
                }
            }
            Err(e) => {
                warn!("Accept connection failed: module=web, error={}", e);
            }
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream, web_root: &Path) -> Result<(), String> {
    let mut buffer = [0_u8; 8192];
    let bytes_read = stream
        .read(&mut buffer)
        .map_err(|e| format!("Failed to read request: {}", e))?;
    if bytes_read == 0 {
        return Ok(());
    }

    let req = String::from_utf8_lossy(&buffer[..bytes_read]);
    let first_line = req
        .lines()
        .next()
        .ok_or_else(|| "Empty request".to_string())?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");

    if method != "GET" {
        write_response(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            b"Only GET is supported",
        )?;
        return Ok(());
    }

    let path = target.split('?').next().unwrap_or("/");
    let relative_path = if path == "/" {
        PathBuf::from("index.html")
    } else {
        sanitize_relative_path(path)?
    };

    let file_path = web_root.join(&relative_path);
    let canonical_root = web_root
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize web root: {}", e))?;
    let canonical_target = file_path.canonicalize().ok();

    let real_target = if let Some(target) = canonical_target {
        if !target.starts_with(&canonical_root) {
            write_response(
                &mut stream,
                403,
                "Forbidden",
                "text/plain; charset=utf-8",
                b"Forbidden",
            )?;
            return Ok(());
        }
        target
    } else {
        write_response(
            &mut stream,
            404,
            "Not Found",
            "text/plain; charset=utf-8",
            b"Not Found",
        )?;
        return Ok(());
    };

    if real_target.is_dir() {
        let index_path = real_target.join("index.html");
        if !index_path.exists() {
            write_response(
                &mut stream,
                404,
                "Not Found",
                "text/plain; charset=utf-8",
                b"Not Found",
            )?;
            return Ok(());
        }

        let bytes = fs::read(&index_path)
            .map_err(|e| format!("Failed to read file {}: {}", index_path.display(), e))?;
        write_response(&mut stream, 200, "OK", content_type(&index_path), &bytes)?;
        return Ok(());
    }

    let bytes = fs::read(&real_target)
        .map_err(|e| format!("Failed to read file {}: {}", real_target.display(), e))?;
    write_response(&mut stream, 200, "OK", content_type(&real_target), &bytes)?;

    Ok(())
}

fn sanitize_relative_path(path: &str) -> Result<PathBuf, String> {
    let mut sanitized = PathBuf::new();
    let no_prefix = path.trim_start_matches('/');
    for comp in Path::new(no_prefix).components() {
        match comp {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("Path traversal is not allowed".to_string()),
            Component::RootDir | Component::Prefix(_) => {
                return Err("Invalid request path".to_string());
            }
        }
    }

    Ok(sanitized)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn write_response(
    stream: &mut TcpStream,
    code: u16,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), String> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        code,
        status,
        content_type,
        body.len()
    );

    stream
        .write_all(header.as_bytes())
        .map_err(|e| format!("Failed to write response header: {}", e))?;
    stream
        .write_all(body)
        .map_err(|e| format!("Failed to write response body: {}", e))?;

    Ok(())
}
