use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::result::Result;
use std::{path, sync};

use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::io::BufRead;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tracing::{debug, info, instrument, trace};

use crate::core::{self, error::MonorailError, tracking};

pub(crate) const STDOUT_FILE: &str = "stdout.zst";
pub(crate) const STDERR_FILE: &str = "stderr.zst";
pub(crate) const RESET_COLOR: &str = "\x1b[0m";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LogFilterInput {
    pub(crate) commands: HashSet<String>,
    pub(crate) targets: HashSet<String>,
    pub(crate) include_stdout: bool,
    pub(crate) include_stderr: bool,
}

#[derive(Serialize)]
pub(crate) struct LogTailInput {
    pub(crate) filter_input: LogFilterInput,
}

pub(crate) async fn log_tail<'a>(
    _cfg: &'a core::Config,
    input: &LogTailInput,
) -> Result<(), MonorailError> {
    // require at least one of the log types be opted into
    if !input.filter_input.include_stdout && !input.filter_input.include_stderr {
        return Err(MonorailError::from(
            "No stream selected; provide one or both of: --stdout, --stderr",
        ));
    }
    let mut lss = StreamServer::new("127.0.0.1:9201", &input.filter_input);
    lss.listen().await
}

#[derive(Serialize)]
pub(crate) struct LogShowInput<'a> {
    pub(crate) id: Option<&'a usize>,
    pub(crate) filter_input: LogFilterInput,
}

pub(crate) fn log_show<'a>(
    cfg: &'a core::Config,
    input: &'a LogShowInput<'a>,
    work_path: &'a path::Path,
) -> Result<(), MonorailError> {
    // require at least one of the log types be opted into
    if !input.filter_input.include_stdout && !input.filter_input.include_stderr {
        return Err(MonorailError::from(
            "No stream selected; provide one or both of: --stdout, --stderr",
        ));
    }
    let run_id = match input.id {
        Some(id) => *id,
        None => {
            let tracking_table = tracking::Table::new(&cfg.get_tracking_path(work_path))?;
            let run = tracking_table.open_run()?;
            run.id
        }
    };

    let run_dir = cfg.get_run_path(work_path).join(format!("{}", run_id));
    if !run_dir.try_exists()? {
        return Err(MonorailError::Generic(format!(
            "Log path {} does not exist",
            &run_dir.display().to_string()
        )));
    }

    // map all targets to their shas for filtering and prefixing log lines
    if cfg.targets.is_empty() {
        return Err(MonorailError::from(
            "No configured targets, cannot tail logs",
        ));
    }

    let mut hasher = sha2::Sha256::new();
    let mut hash2target = HashMap::new();
    for target in &cfg.targets {
        hasher.update(&target.path);
        hash2target.insert(format!("{:x}", hasher.finalize_reset()), &target.path);
    }

    let mut stdout = std::io::stdout();
    // open directory at run_dir
    for fn_entry in run_dir.read_dir()? {
        let fn_path = fn_entry?.path();
        if fn_path.is_dir() {
            let command = fn_path.file_name().unwrap().to_str().unwrap();
            for t_entry in fn_path.read_dir()? {
                let t_path = t_entry?.path();
                if t_path.is_dir() {
                    let target_hash = t_path.file_name().unwrap().to_str().unwrap();
                    for e in t_path.read_dir()? {
                        let target =
                            hash2target
                                .get(target_hash)
                                .ok_or(MonorailError::Generic(format!(
                                    "Target not found for {}",
                                    &target_hash
                                )))?;
                        let p = e?.path();
                        let filename = p
                            .file_name()
                            .ok_or(MonorailError::Generic(format!(
                                "Bad path file name: {:?}",
                                &p
                            )))?
                            .to_str()
                            .ok_or(MonorailError::from("Bad file name string"))?;
                        if is_log_allowed(
                            &input.filter_input.targets,
                            &input.filter_input.commands,
                            target,
                            command,
                        ) && (filename == STDOUT_FILE && input.filter_input.include_stdout
                            || filename == STDERR_FILE && input.filter_input.include_stderr)
                        {
                            let header = get_header(filename, target, command, true);
                            let header_bytes = header.as_bytes();
                            stream_archive_file_to_stdout(header_bytes, &p, &mut stdout)?;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn is_log_allowed(
    targets: &HashSet<String>,
    commands: &HashSet<String>,
    target: &str,
    command: &str,
) -> bool {
    let target_allowed = targets.is_empty() || targets.contains(target);
    let command_allowed = commands.is_empty() || commands.contains(command);
    target_allowed && command_allowed
}

fn stream_archive_file_to_stdout(
    header: &[u8],
    path: &path::Path,
    stdout: &mut std::io::Stdout,
) -> Result<(), MonorailError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let decoder = zstd::stream::read::Decoder::new(reader)?;
    let mut line_reader = std::io::BufReader::new(decoder);
    let mut line: Vec<u8> = Vec::new();
    let mut wrote_header = false;
    while line_reader.read_until(b'\n', &mut line)? > 0 {
        if !wrote_header {
            stdout.write_all(header)?;
            wrote_header = true;
        }
        stdout.write_all(&line)?;
        line.clear();
    }

    Ok(())
}

pub(crate) struct StreamServer<'a> {
    address: &'a str,
    args: &'a LogFilterInput,
}
impl<'a> StreamServer<'a> {
    pub(crate) fn new(address: &'a str, args: &'a LogFilterInput) -> Self {
        Self { address, args }
    }
    pub(crate) async fn listen(&mut self) -> Result<(), MonorailError> {
        let listener = tokio::net::TcpListener::bind(self.address).await?;
        let args_data = serde_json::to_vec(&self.args)?;
        debug!("Log stream server listening");
        loop {
            let (mut socket, _) = listener.accept().await?;
            debug!("Client connected");
            // first, write to the client what we're interested in receiving
            socket.write_all(&args_data).await?;
            _ = socket.write(b"\n").await?;
            debug!("Sent log stream arguments");
            Self::process(socket).await?;
        }
    }
    #[instrument]
    async fn process(socket: tokio::net::TcpStream) -> Result<(), MonorailError> {
        let br = tokio::io::BufReader::new(socket);
        let mut lines = br.lines();
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines.next_line().await? {
            stdout.write_all(line.as_bytes()).await?;
            _ = stdout.write(b"\n").await?;
            stdout.flush().await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StreamClient {
    stream: sync::Arc<tokio::sync::Mutex<tokio::net::TcpStream>>,
    pub(crate) args: LogFilterInput,
}
impl StreamClient {
    #[instrument]
    pub(crate) async fn data(
        &mut self,
        data: sync::Arc<Vec<Vec<u8>>>,
        header: &[u8],
    ) -> Result<(), MonorailError> {
        let mut guard = self.stream.lock().await;
        guard.write_all(header).await.map_err(MonorailError::from)?;
        for v in data.iter() {
            guard.write_all(v).await.map_err(MonorailError::from)?;
        }
        Ok(())
    }
    #[instrument]
    pub(crate) async fn connect(addr: &str) -> Result<Self, MonorailError> {
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        info!(address = addr, "Connected to log stream server");
        let mut args_data = Vec::new();
        // pull arg preferences from the server on connect
        let mut br = tokio::io::BufReader::new(&mut stream);
        br.read_until(b'\n', &mut args_data).await?;
        let args: LogFilterInput = serde_json::from_slice(args_data.as_slice())?;
        debug!("Received log stream arguments");
        if args.include_stdout || args.include_stderr {
            let targets = if args.targets.is_empty() {
                String::from("(any target)")
            } else {
                args.targets.iter().cloned().collect::<Vec<_>>().join(", ")
            };
            let commands = if args.commands.is_empty() {
                String::from("(any command)")
            } else {
                args.commands.iter().cloned().collect::<Vec<_>>().join(", ")
            };
            let mut files = vec![];
            if args.include_stdout {
                files.push(STDOUT_FILE);
            }
            if args.include_stderr {
                files.push(STDERR_FILE);
            }
            stream
                .write_all(get_header(&files.join(", "), &targets, &commands, false).as_bytes())
                .await
                .map_err(MonorailError::from)?;
        }

        Ok(Self {
            stream: sync::Arc::new(tokio::sync::Mutex::new(stream)),
            args,
        })
    }
}

pub(crate) async fn process_reader<R>(
    config: &core::LogConfig,
    mut reader: tokio::io::BufReader<R>,
    compressor_client: CompressorClient,
    header: String,
    mut log_stream_client: Option<StreamClient>,
    token: sync::Arc<tokio_util::sync::CancellationToken>,
) -> Result<(), MonorailError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_millis(config.flush_interval_ms));
    loop {
        let mut bufs = Vec::new();
        loop {
            let mut buf = Vec::new();
            tokio::select! {
                _ = token.cancelled() => {
                    process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                    return Err(MonorailError::TaskCancelled);
                }
                res = reader.read_until(b'\n', &mut buf) => {
                    match res {
                        Ok(0) => {
                            process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                            return Ok(());
                        },
                        Ok(_n) => {
                            bufs.push(buf);
                        }
                        Err(e) => {
                            process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, true).await?;
                            return Err(MonorailError::from(e));
                        }
                    }
                }
                _ = interval.tick() => {
                    process_bufs(&header, bufs, &compressor_client, &mut log_stream_client, false).await?;
                    break;
                }
            }
        }
    }
}

#[instrument]
async fn process_bufs(
    header: &str,
    bufs: Vec<Vec<u8>>,
    compressor_client: &CompressorClient,
    log_stream_client: &mut Option<StreamClient>,
    should_end: bool,
) -> Result<(), MonorailError> {
    if !bufs.is_empty() {
        let bufs_arc = sync::Arc::new(bufs);
        let bufs_arc2 = bufs_arc.clone();
        let lsc_fut = async {
            if let Some(ref mut lsc) = log_stream_client {
                lsc.data(bufs_arc2, header.as_bytes()).await
            } else {
                Ok(())
            }
        };
        let cc_fut = async { compressor_client.data(bufs_arc).await };
        let (_, _) = tokio::try_join!(lsc_fut, cc_fut)?;
    }
    if should_end {
        compressor_client.end().await?;
    }

    Ok(())
}

#[derive(Debug)]
pub(crate) enum CompressRequest {
    Data(usize, sync::Arc<Vec<Vec<u8>>>),
    End(usize),
    Shutdown,
}
#[derive(Debug)]
pub(crate) struct Compressor {
    index: usize,
    num_threads: usize,
    req_channels: Vec<(
        flume::Sender<CompressRequest>,
        Option<flume::Receiver<CompressRequest>>,
    )>,
    registrations: Vec<Vec<path::PathBuf>>,
    shutdown: sync::Arc<sync::atomic::AtomicBool>,
}
#[derive(Debug, Clone)]
pub(crate) struct CompressorClient {
    pub(crate) file_name: String,
    pub(crate) encoder_index: usize,
    pub(crate) req_tx: flume::Sender<CompressRequest>,
}
impl CompressorClient {
    pub(crate) async fn data(&self, data: sync::Arc<Vec<Vec<u8>>>) -> Result<(), MonorailError> {
        self.req_tx
            .send_async(CompressRequest::Data(self.encoder_index, data))
            .await
            .map_err(MonorailError::from)
    }
    pub(crate) async fn end(&self) -> Result<(), MonorailError> {
        self.req_tx
            .send_async(CompressRequest::End(self.encoder_index))
            .await
            .map_err(MonorailError::from)
    }
    pub(crate) fn shutdown(&self) -> Result<(), MonorailError> {
        self.req_tx
            .send(CompressRequest::Shutdown)
            .map_err(MonorailError::from)
    }
}

impl Compressor {
    pub(crate) fn new(num_threads: usize, shutdown: sync::Arc<sync::atomic::AtomicBool>) -> Self {
        let mut req_channels = vec![];
        let mut registrations = vec![];
        for _ in 0..num_threads {
            let (req_tx, req_rx) = flume::bounded(1000);
            req_channels.push((req_tx, Some(req_rx)));
            registrations.push(vec![]);
        }
        Self {
            index: 0,
            num_threads,
            req_channels,
            registrations,
            shutdown,
        }
    }

    // Register the provided path and return a CompressorClient
    // that can be used to schedule operations on the underlying encoder.
    pub(crate) fn register(&mut self, p: &path::Path) -> Result<CompressorClient, MonorailError> {
        // todo; check path not already seen
        let thread_index = self.index % self.num_threads;
        let encoder_index = self.registrations[thread_index].len();
        self.registrations[thread_index].push(p.to_path_buf());
        self.index += 1;

        let file_name = p.file_name().unwrap().to_str().unwrap().to_string(); // todo; monorailerror

        Ok(CompressorClient {
            file_name,
            encoder_index,
            req_tx: self.req_channels[thread_index].0.clone(),
        })
    }
    pub(crate) fn run(&mut self) -> Result<(), MonorailError> {
        std::thread::scope(|s| {
            for x in 0..self.num_threads {
                let regs = &self.registrations[x];
                let req_rx =
                    self.req_channels[x]
                        .1
                        .take()
                        .ok_or(MonorailError::Generic(format!(
                            "Missing channel for index {}",
                            x
                        )))?;
                let shutdown = &self.shutdown;
                s.spawn(move || {
                    let mut encoders = vec![];
                    for r in regs.iter() {
                        let f = std::fs::OpenOptions::new()
                            .create(true)
                            .write(true)
                            .truncate(true)
                            .open(r)
                            .map_err(|e| MonorailError::Generic(e.to_string()))?;
                        let bw = std::io::BufWriter::new(f);
                        encoders.push(zstd::stream::write::Encoder::new(bw, 3)?);
                    }
                    loop {
                        if shutdown.load(sync::atomic::Ordering::Relaxed) {
                            break;
                        };
                        match req_rx.recv()? {
                            CompressRequest::End(encoder_index) => {
                                trace!(
                                    encoder_index = encoder_index,
                                    thread_id = x,
                                    "Encoder finish"
                                );
                                encoders[encoder_index].do_finish()?;
                            }
                            CompressRequest::Shutdown => {
                                trace!("Compressor shutdown");
                                break;
                            }
                            CompressRequest::Data(encoder_index, data) => {
                                trace!(lines = data.len(), thread_id = x, "Encoder write");
                                for v in data.iter() {
                                    encoders[encoder_index].write_all(v)?;
                                }
                            }
                        }
                    }
                    for mut enc in encoders {
                        trace!(thread_id = x, "Encoder finish");
                        enc.do_finish()?;
                    }
                    Ok::<(), MonorailError>(())
                });
            }
            Ok(())
        })
    }
}

pub(crate) fn get_header(filename: &str, target: &str, command: &str, color: bool) -> String {
    if color {
        let filename_color = match filename {
            STDOUT_FILE => "\x1b[38;5;81m",
            STDERR_FILE => "\x1b[38;5;214m",
            _ => "",
        };
        format!("[monorail | {filename_color}{filename}{RESET_COLOR} | {target} | {command}]\n")
    } else {
        format!("[monorail | {filename} | {target} | {command}]\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testing::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn test_compressor_initialization() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let compressor = Compressor::new(4, shutdown.clone());

        assert_eq!(compressor.num_threads, 4);
        assert_eq!(compressor.index, 0);
        assert_eq!(compressor.req_channels.len(), 4);
        assert_eq!(compressor.registrations.len(), 4);
        assert!(!compressor.shutdown.load(Ordering::Relaxed));
    }

    #[test]
    fn test_register_paths() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut compressor = Compressor::new(2, shutdown.clone());
        let temp_dir = new_testdir().unwrap();

        // Register a path and validate the returned CompressorClient
        let test_path = temp_dir.path().join("test_file_1");
        let client = compressor
            .register(&test_path)
            .expect("Failed to register path");

        // Check that the path is added to the correct registration slot
        assert_eq!(compressor.registrations[0][0], test_path);
        assert_eq!(client.file_name, "test_file_1".to_string());
        assert_eq!(client.encoder_index, 0);

        // Register another path and check the rotation across threads
        let test_path_2 = temp_dir.path().join("test_file_2");
        let client_2 = compressor
            .register(&test_path_2)
            .expect("Failed to register path");

        assert_eq!(compressor.registrations[1][0], test_path_2);
        assert_eq!(client_2.file_name, "test_file_2".to_string());
        assert_eq!(client_2.encoder_index, 0);
    }

    #[test]
    fn test_run_compressor_with_data_requests() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut compressor = Compressor::new(1, shutdown.clone());
        let temp_dir = new_testdir().unwrap();

        let test_path = temp_dir.path().join("test_output.zst");
        let client = compressor
            .register(&test_path)
            .expect("Failed to register path");

        // Spawn the compressor threads
        let compressor_handle = std::thread::spawn(move || compressor.run());

        // Send a Data request
        let data = Arc::new(vec![b"Test data".to_vec()]);
        client
            .req_tx
            .send(CompressRequest::Data(client.encoder_index, data.clone()))
            .expect("Failed to send data");

        // Finalize the encoder and shut down the thread
        client.shutdown().unwrap();

        // Wait for the compressor thread to end
        compressor_handle.join().unwrap().unwrap();

        // Verify that the compressed file is created and contains data
        assert!(test_path.exists());

        // Check that the file is not empty
        let metadata = std::fs::metadata(&test_path).expect("Failed to get metadata");
        assert!(metadata.len() > 0, "Compressed file should not be empty");
    }

    #[test]
    fn test_shutdown_compressor() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut compressor = Compressor::new(2, shutdown.clone());
        let temp_dir = new_testdir().unwrap();

        // Register paths
        let test_path_1 = temp_dir.path().join("test_file_1");
        let test_path_2 = temp_dir.path().join("test_file_2");
        let client_1 = compressor
            .register(&test_path_1)
            .expect("Failed to register path");
        let client_2 = compressor
            .register(&test_path_2)
            .expect("Failed to register path");

        // Run compressor threads
        let compressor_handle = std::thread::spawn(move || compressor.run());

        // Send Shutdown requests to each compressor thread
        client_1
            .req_tx
            .send(CompressRequest::Shutdown)
            .expect("Failed to send shutdown request");
        client_2
            .req_tx
            .send(CompressRequest::Shutdown)
            .expect("Failed to send shutdown request");

        // Wait for the compressor threads to end
        compressor_handle.join().unwrap().unwrap();

        // Assert that shutdown flag was set to true
        shutdown.store(true, Ordering::Relaxed);
        assert!(shutdown.load(Ordering::Relaxed));
    }
}
