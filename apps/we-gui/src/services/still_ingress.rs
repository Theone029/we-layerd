use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use we_core::ingress::{
    StillIngressResponseHeader, StillIngressStatus, STILL_INGRESS_MAX_HEADER_BYTES,
    STILL_INGRESS_REQUEST_LINE,
};

const DEFAULT_SOCKET: &str = "/run/wallpaper-private-ingress/image.sock";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StillIngressSelection {
    pub(crate) source_path: PathBuf,
    pub(crate) display_name: String,
}

pub(crate) async fn pick() -> Result<Option<StillIngressSelection>, String> {
    pick_sync(&socket_path(), &staging_root())
}

pub(crate) fn discard_staged(path: &Path) {
    let root = staging_root();

    if path.parent() == Some(root.as_path()) {
        let _ = fs::remove_file(path);
    }
}

fn socket_path() -> PathBuf {
    env::var_os("WE_IMAGE_INGRESS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

fn staging_root() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("we-gui")
        .join("still-ingress")
}

fn pick_sync(
    socket_path: &Path,
    staging_root: &Path,
) -> Result<Option<StillIngressSelection>, String> {
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        format!("secure image picker is unavailable at {}: {error}", socket_path.display())
    })?;

    stream
        .write_all(STILL_INGRESS_REQUEST_LINE.as_bytes())
        .map_err(|error| format!("failed to request secure image picker: {error}"))?;

    let mut reader = BufReader::new(stream);

    let line = read_bounded_line(&mut reader, STILL_INGRESS_MAX_HEADER_BYTES)?;

    let header = StillIngressResponseHeader::decode_line(&line)?;

    match header.status {
        StillIngressStatus::Cancel => Ok(None),

        StillIngressStatus::Error => {
            Err(header.message.unwrap_or_else(|| "secure image picker failed".to_string()))
        }

        StillIngressStatus::Ok => {
            let display_name = header
                .name
                .ok_or_else(|| "secure image picker omitted the file name".to_string())?;

            fs::create_dir_all(staging_root).map_err(|error| {
                format!(
                    "failed to create private image staging {}: {error}",
                    staging_root.display()
                )
            })?;

            private_dir_permissions(staging_root)?;

            let (mut file, path) = create_staging_file(staging_root)?;

            let result = receive_payload(&mut reader, &mut file, header.len);

            if let Err(error) = result {
                drop(file);
                let _ = fs::remove_file(&path);
                return Err(error);
            }

            file.sync_all().map_err(|error| {
                format!("failed to sync private image staging {}: {error}", path.display())
            })?;

            Ok(Some(StillIngressSelection { source_path: path, display_name }))
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_len: usize) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();

    loop {
        let available = reader
            .fill_buf()
            .map_err(|error| format!("failed to read secure image picker response: {error}"))?;

        if available.is_empty() {
            return Err("secure image picker closed before sending a header".to_string());
        }

        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(available.len());

        if result.len().saturating_add(count) > max_len {
            return Err("secure image picker header exceeds limit".to_string());
        }

        result.extend_from_slice(&available[..count]);
        reader.consume(count);

        if result.last() == Some(&b'\n') {
            return Ok(result);
        }
    }
}

fn receive_payload<R: Read>(reader: &mut R, file: &mut File, length: u64) -> Result<(), String> {
    let mut remaining = length;
    let mut buffer = [0_u8; 64 * 1024];

    while remaining > 0 {
        let wanted =
            usize::try_from(remaining.min(buffer.len() as u64)).expect("bounded buffer length");

        let count = reader
            .read(&mut buffer[..wanted])
            .map_err(|error| format!("failed to receive selected image: {error}"))?;

        if count == 0 {
            return Err(format!(
                "secure image picker truncated payload: \
                 expected {length} bytes"
            ));
        }

        file.write_all(&buffer[..count])
            .map_err(|error| format!("failed to stage selected image: {error}"))?;

        remaining -= count as u64;
    }

    Ok(())
}

fn create_staging_file(root: &Path) -> Result<(File, PathBuf), String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock unavailable for image staging: {error}"))?
        .as_nanos();

    for attempt in 0..1000_u32 {
        let path =
            root.join(format!("ingress-{}-{timestamp}-{attempt}.upload", std::process::id(),));

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                private_file_permissions(&path)?;
                return Ok((file, path));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "failed to create private image staging {}: {error}",
                    path.display()
                ));
            }
        }
    }

    Err("could not allocate private image staging file".to_string())
}

#[cfg(unix)]
fn private_dir_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
        format!("failed to secure image staging directory {}: {error}", path.display())
    })
}

#[cfg(unix)]
fn private_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure image staging file {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        path::{Path, PathBuf},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use we_core::ingress::{
        StillIngressResponseHeader, STILL_INGRESS_MAX_PAYLOAD_BYTES, STILL_INGRESS_REQUEST_LINE,
    };

    use super::pick_sync;

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();

        let path = std::env::temp_dir()
            .join(format!("we-gui-ingress-{label}-{}-{nonce}", std::process::id()));

        fs::create_dir_all(&path).expect("temp root");
        path
    }

    fn spawn_server(
        socket: &Path,
        header: StillIngressResponseHeader,
        payload: Vec<u8>,
    ) -> thread::JoinHandle<()> {
        let listener = UnixListener::bind(socket).expect("bind fake broker");

        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept client");

            let mut request = String::new();
            BufReader::new(stream.try_clone().expect("clone stream"))
                .read_line(&mut request)
                .expect("read request");

            assert_eq!(request, STILL_INGRESS_REQUEST_LINE);

            stream.write_all(&header.encode_line().expect("encode header")).expect("write header");

            stream.write_all(&payload).expect("write payload");
        })
    }

    #[test]
    fn selected_bytes_cross_only_through_private_staging() {
        let root = temp_root("success");
        let socket = root.join("broker.sock");
        let staging = root.join("staging");

        let payload = b"fake-png-payload".to_vec();

        let server = spawn_server(
            &socket,
            StillIngressResponseHeader::ok("Vacation Photo.png", payload.len() as u64),
            payload.clone(),
        );

        let selection =
            pick_sync(&socket, &staging).expect("pick succeeds").expect("selection exists");

        assert_eq!(selection.display_name, "Vacation Photo.png");
        assert_eq!(fs::read(&selection.source_path).expect("read staged payload"), payload);
        assert_eq!(selection.source_path.parent(), Some(staging.as_path()));

        server.join().expect("server");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn cancellation_creates_no_staged_file() {
        let root = temp_root("cancel");
        let socket = root.join("broker.sock");
        let staging = root.join("staging");

        let server = spawn_server(&socket, StillIngressResponseHeader::cancel(), Vec::new());

        assert!(pick_sync(&socket, &staging).expect("cancel response").is_none());

        assert!(!staging.exists());

        server.join().expect("server");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn oversized_payload_is_rejected_before_staging() {
        let root = temp_root("oversize");
        let socket = root.join("broker.sock");
        let staging = root.join("staging");

        let server = spawn_server(
            &socket,
            StillIngressResponseHeader::ok("too-large.png", STILL_INGRESS_MAX_PAYLOAD_BYTES + 1),
            Vec::new(),
        );

        let error = pick_sync(&socket, &staging).expect_err("oversized response must fail");

        assert!(error.contains("exceeds"));
        assert!(!staging.exists());

        server.join().expect("server");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn truncated_payload_is_removed() {
        let root = temp_root("truncated");
        let socket = root.join("broker.sock");
        let staging = root.join("staging");

        let server = spawn_server(
            &socket,
            StillIngressResponseHeader::ok("broken.png", 100),
            b"short".to_vec(),
        );

        let error = pick_sync(&socket, &staging).expect_err("truncated response must fail");

        assert!(error.contains("truncated"));

        if staging.exists() {
            assert_eq!(fs::read_dir(&staging).expect("read staging").count(), 0);
        }

        server.join().expect("server");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
