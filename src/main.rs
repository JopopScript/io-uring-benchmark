use chrono::Utc;
use std::{
    env, io,
    net::{Shutdown as StdShutdown, SocketAddr},
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener as TokioTcpListener, TcpStream as TokioTcpStream},
};
use tokio_uring::{
    fs::File,
    net::{TcpListener as UringTcpListener, TcpStream as UringTcpStream},
};
use uuid::Uuid;

const SOURCE_FILE: &str = "data/source.txt";
const OUTPUT_DIR: &str = "data/output";
const READ_BUFFER_SIZE: usize = 4 * 1024;
const HTTP_REQUEST_BUFFER_SIZE: usize = 8 * 1024;

#[derive(Clone, Copy, Debug)]
enum Transport {
    IoUring,
    Epoll,
}

impl Transport {
    fn from_env() -> io::Result<Self> {
        let raw = env::var("TRANSPORT").unwrap_or_else(|_| "io_uring".to_owned());

        match raw.trim().to_ascii_lowercase().as_str() {
            "io_uring" | "uring" => Ok(Self::IoUring),
            "epoll" | "tokio" => Ok(Self::Epoll),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("TRANSPORT must be io_uring or epoll, got {raw:?}"),
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::IoUring => "io_uring",
            Self::Epoll => "epoll",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ServerOptions {
    sync_writes: bool,
}

#[derive(Clone, Copy, Debug)]
struct AppConfig {
    address: SocketAddr,
    transport: Transport,
    options: ServerOptions,
}

fn main() -> io::Result<()> {
    prepare_files()?;

    let bind_address = env::var("BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let address: SocketAddr = bind_address.parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid BIND_ADDRESS {bind_address:?}: {error}"),
        )
    })?;

    let config = AppConfig {
        address,
        transport: Transport::from_env()?,
        options: ServerOptions {
            sync_writes: sync_writes_enabled(),
        },
    };

    println!("transport: {}", config.transport.as_str());
    println!("listening on http://{}", config.address);
    println!("source file: {SOURCE_FILE}");
    println!("output directory: {OUTPUT_DIR}");
    println!(
        "sync writes: {}",
        if config.options.sync_writes {
            "enabled"
        } else {
            "disabled"
        }
    );

    match config.transport {
        Transport::IoUring => run_io_uring_server(config.address, config.options),
        Transport::Epoll => run_epoll_server(config.address, config.options),
    }
}

fn prepare_files() -> io::Result<()> {
    std::fs::create_dir_all(OUTPUT_DIR)?;

    if !Path::new(SOURCE_FILE).exists() {
        let mut content = Vec::with_capacity(READ_BUFFER_SIZE);
        while content.len() < READ_BUFFER_SIZE {
            content.extend_from_slice(b"io_uring_benchmark source data: 0123456789abcdef\n");
        }
        content.truncate(READ_BUFFER_SIZE);
        std::fs::write(SOURCE_FILE, content)?;
    }

    Ok(())
}

fn run_io_uring_server(address: SocketAddr, options: ServerOptions) -> io::Result<()> {
    tokio_uring::builder()
        .entries(1024)
        .start(async move { accept_io_uring_connections(address, options).await })
}

async fn accept_io_uring_connections(
    address: SocketAddr,
    options: ServerOptions,
) -> io::Result<()> {
    let listener = UringTcpListener::bind(address)?;

    loop {
        let (stream, peer_address) = listener.accept().await?;

        if let Err(error) = stream.set_nodelay(true) {
            eprintln!("failed to set TCP_NODELAY for {peer_address}: {error}");
        }

        tokio_uring::spawn(async move {
            if let Err(error) = handle_io_uring_connection(stream, options).await {
                eprintln!("request from {peer_address} failed: {error}");
            }
        });
    }
}

async fn handle_io_uring_connection(
    stream: UringTcpStream,
    options: ServerOptions,
) -> io::Result<()> {
    let request_buffer = vec![0_u8; HTTP_REQUEST_BUFFER_SIZE];
    let (read_result, request_buffer) = stream.read(request_buffer).await;
    let bytes_read = read_result?;

    if bytes_read == 0 {
        return Ok(());
    }

    let request = &request_buffer[..bytes_read];

    if is_health_request(request) {
        send_io_uring_empty_response(&stream, "204 No Content").await?;
        let _ = stream.shutdown(StdShutdown::Both);
        return Ok(());
    }

    if !is_work_request(request) {
        send_io_uring_empty_response(&stream, "404 Not Found").await?;
        let _ = stream.shutdown(StdShutdown::Both);
        return Ok(());
    }

    match copy_source_with_timestamp_io_uring(options).await {
        Ok(()) => send_io_uring_empty_response(&stream, "204 No Content").await?,
        Err(error) => {
            eprintln!("file operation failed: {error}");
            send_io_uring_empty_response(&stream, "500 Internal Server Error").await?;
        }
    }

    let _ = stream.shutdown(StdShutdown::Both);
    Ok(())
}

fn run_epoll_server(address: SocketAddr, options: ServerOptions) -> io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .build()?
        .block_on(async move { accept_epoll_connections(address, options).await })
}

async fn accept_epoll_connections(address: SocketAddr, options: ServerOptions) -> io::Result<()> {
    let listener = TokioTcpListener::bind(address).await?;

    loop {
        let (stream, peer_address) = listener.accept().await?;

        if let Err(error) = stream.set_nodelay(true) {
            eprintln!("failed to set TCP_NODELAY for {peer_address}: {error}");
        }

        tokio::spawn(async move {
            if let Err(error) = handle_epoll_connection(stream, options).await {
                eprintln!("request from {peer_address} failed: {error}");
            }
        });
    }
}

async fn handle_epoll_connection(
    mut stream: TokioTcpStream,
    options: ServerOptions,
) -> io::Result<()> {
    let mut request_buffer = vec![0_u8; HTTP_REQUEST_BUFFER_SIZE];
    let bytes_read = stream.read(&mut request_buffer).await?;

    if bytes_read == 0 {
        return Ok(());
    }

    let request = &request_buffer[..bytes_read];

    if is_health_request(request) {
        send_epoll_empty_response(&mut stream, "204 No Content").await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    if !is_work_request(request) {
        send_epoll_empty_response(&mut stream, "404 Not Found").await?;
        let _ = stream.shutdown().await;
        return Ok(());
    }

    match copy_source_with_timestamp_epoll(options).await {
        Ok(()) => send_epoll_empty_response(&mut stream, "204 No Content").await?,
        Err(error) => {
            eprintln!("file operation failed: {error}");
            send_epoll_empty_response(&mut stream, "500 Internal Server Error").await?;
        }
    }

    let _ = stream.shutdown().await;
    Ok(())
}

fn is_health_request(request: &[u8]) -> bool {
    request.starts_with(b"GET /health ")
}

fn is_work_request(request: &[u8]) -> bool {
    request.starts_with(b"GET /work ") || request.starts_with(b"POST /work ")
}

async fn copy_source_with_timestamp_io_uring(options: ServerOptions) -> io::Result<()> {
    let source = File::open(SOURCE_FILE).await?;
    let read_buffer = vec![0_u8; READ_BUFFER_SIZE];

    let (read_result, mut read_buffer) = source.read_at(read_buffer, 0).await;
    let bytes_read = read_result?;
    source.close().await?;

    read_buffer.truncate(bytes_read);
    append_timestamp(&mut read_buffer);

    let output_path = output_path();
    let output = File::create(&output_path).await?;

    let (write_result, _buffer) = output.write_all_at(read_buffer, 0).await;
    write_result?;

    if options.sync_writes {
        output.sync_data().await?;
    }

    output.close().await?;
    Ok(())
}

async fn copy_source_with_timestamp_epoll(options: ServerOptions) -> io::Result<()> {
    let mut source = tokio::fs::File::open(SOURCE_FILE).await?;
    let mut read_buffer = vec![0_u8; READ_BUFFER_SIZE];
    let bytes_read = source.read(&mut read_buffer).await?;

    read_buffer.truncate(bytes_read);
    append_timestamp(&mut read_buffer);

    let output_path = output_path();
    let mut output = tokio::fs::File::create(&output_path).await?;
    output.write_all(&read_buffer).await?;
    output.flush().await?;

    if options.sync_writes {
        output.sync_data().await?;
    }

    Ok(())
}

fn output_path() -> PathBuf {
    Path::new(OUTPUT_DIR).join(format!("{}.txt", Uuid::new_v4()))
}

fn append_timestamp(buffer: &mut Vec<u8>) {
    let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    buffer.extend_from_slice(b"\n");
    buffer.extend_from_slice(timestamp.as_bytes());
    buffer.extend_from_slice(b"\n");
}

fn sync_writes_enabled() -> bool {
    matches!(
        env::var("SYNC_WRITES").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE")
    )
}

fn empty_response(status: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         \r\n"
    )
    .into_bytes()
}

async fn send_io_uring_empty_response(stream: &UringTcpStream, status: &str) -> io::Result<()> {
    let response = empty_response(status);

    let (write_result, _response) = stream.write_all(response).await;
    write_result
}

async fn send_epoll_empty_response(stream: &mut TokioTcpStream, status: &str) -> io::Result<()> {
    let response = empty_response(status);
    stream.write_all(&response).await
}
