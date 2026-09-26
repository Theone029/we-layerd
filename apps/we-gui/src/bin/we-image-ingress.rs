use std::{
    env,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use we_core::ingress::{
    MediaIngressResponseHeader, MEDIA_INGRESS_MAX_PAYLOAD_BYTES, MEDIA_INGRESS_REQUEST_LINE,
};

const DEFAULT_SOCKET: &str = "/run/wallpaper-private-ingress/image.sock";

fn main() {
    if let Err(error) = run() {
        eprintln!("we-image-ingress: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let socket = parse_socket_path()?;

    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("cannot create socket directory {}: {error}", parent.display())
        })?;
    }

    if socket.exists() {
        fs::remove_file(&socket)
            .map_err(|error| format!("cannot remove stale socket {}: {error}", socket.display()))?;
    }

    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("cannot bind picker socket {}: {error}", socket.display()))?;

    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660))
        .map_err(|error| format!("cannot secure picker socket {}: {error}", socket.display()))?;

    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(error) = handle_request(&mut stream) {
                    eprintln!("we-image-ingress request failed: {error}");
                }
            }
            Err(error) => {
                eprintln!("we-image-ingress accept failed: {error}");
            }
        }
    }

    Ok(())
}

fn parse_socket_path() -> Result<PathBuf, String> {
    let mut args = env::args_os().skip(1);

    match args.next() {
        None => Ok(env::var_os("WE_IMAGE_INGRESS_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))),

        Some(flag) if flag == "--socket" => {
            let path = args.next().ok_or_else(|| "--socket requires a path".to_string())?;

            if args.next().is_some() {
                return Err("unexpected arguments after socket path".to_string());
            }

            Ok(PathBuf::from(path))
        }

        Some(other) => Err(format!("unknown argument: {}", other.to_string_lossy())),
    }
}

fn handle_request(stream: &mut UnixStream) -> Result<(), String> {
    let mut request = String::new();

    BufReader::new(
        stream.try_clone().map_err(|error| format!("cannot clone request socket: {error}"))?,
    )
    .read_line(&mut request)
    .map_err(|error| format!("cannot read picker request: {error}"))?;

    if request != MEDIA_INGRESS_REQUEST_LINE {
        send_header(stream, MediaIngressResponseHeader::error("unsupported picker request"))?;

        return Ok(());
    }

    let selected = rfd::FileDialog::new()
        .set_title("Choose wallpaper media")
        .add_filter(
            "Supported media",
            &["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm", "mov", "mkv"],
        )
        .add_filter("Still images", &["png", "jpg", "jpeg", "webp"])
        .add_filter("Animated/video", &["gif", "mp4", "webm", "mov", "mkv"])
        .pick_file();

    let Some(path) = selected else {
        send_header(stream, MediaIngressResponseHeader::cancel())?;
        return Ok(());
    };

    send_selected_file(stream, &path)
}

fn send_selected_file(stream: &mut UnixStream, path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|error| {
        send_error_best_effort(stream, format!("cannot open selected media: {error}"));

        format!("cannot open selected media {}: {error}", path.display())
    })?;

    let metadata = file.metadata().map_err(|error| {
        send_error_best_effort(stream, format!("cannot inspect selected media: {error}"));

        format!("cannot inspect selected media {}: {error}", path.display())
    })?;

    if !metadata.is_file() {
        send_header(
            stream,
            MediaIngressResponseHeader::error("selected media is not a regular file"),
        )?;
        return Ok(());
    }

    let length = metadata.len();

    if length == 0 {
        send_header(stream, MediaIngressResponseHeader::error("selected media is empty"))?;
        return Ok(());
    }

    if length > MEDIA_INGRESS_MAX_PAYLOAD_BYTES {
        send_header(
            stream,
            MediaIngressResponseHeader::error(format!(
                "selected media exceeds the {} GiB transport limit",
                MEDIA_INGRESS_MAX_PAYLOAD_BYTES / (1024 * 1024 * 1024)
            )),
        )?;
        return Ok(());
    }

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().chars().take(240).collect())
        .filter(|name: &String| !name.trim().is_empty())
        .unwrap_or_else(|| "wallpaper-media".to_string());

    send_header(stream, MediaIngressResponseHeader::ok(name, length))?;

    let mut limited = std::io::Read::take(&mut file, length);

    let copied = io::copy(&mut limited, stream)
        .map_err(|error| format!("failed to transfer selected media: {error}"))?;

    if copied != length {
        return Err(format!(
            "selected media changed during transfer: \
             expected {length} bytes, sent {copied}"
        ));
    }

    stream.flush().map_err(|error| format!("failed to flush selected media: {error}"))
}

fn send_header(stream: &mut UnixStream, header: MediaIngressResponseHeader) -> Result<(), String> {
    stream
        .write_all(
            &header
                .encode_line()
                .map_err(|error| format!("failed to encode picker response: {error}"))?,
        )
        .map_err(|error| format!("failed to send picker response: {error}"))
}

fn send_error_best_effort(stream: &mut UnixStream, message: String) {
    let _ = send_header(
        stream,
        MediaIngressResponseHeader::error(message.chars().take(512).collect::<String>()),
    );
}
