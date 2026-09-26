use serde::{Deserialize, Serialize};

pub const MEDIA_INGRESS_PROTOCOL_VERSION: u32 = 2;
pub const MEDIA_INGRESS_MAX_PAYLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MEDIA_INGRESS_MAX_HEADER_BYTES: usize = 8 * 1024;

pub const MEDIA_INGRESS_REQUEST_LINE: &str = "{\"version\":2,\"operation\":\"pick_media\"}\n";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaIngressStatus {
    Ok,
    Cancel,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaIngressResponseHeader {
    pub version: u32,
    pub status: MediaIngressStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl MediaIngressResponseHeader {
    pub fn ok(name: impl Into<String>, len: u64) -> Self {
        Self {
            version: MEDIA_INGRESS_PROTOCOL_VERSION,
            status: MediaIngressStatus::Ok,
            name: Some(name.into()),
            len,
            message: None,
        }
    }

    pub fn cancel() -> Self {
        Self {
            version: MEDIA_INGRESS_PROTOCOL_VERSION,
            status: MediaIngressStatus::Cancel,
            name: None,
            len: 0,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            version: MEDIA_INGRESS_PROTOCOL_VERSION,
            status: MediaIngressStatus::Error,
            name: None,
            len: 0,
            message: Some(message.into()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != MEDIA_INGRESS_PROTOCOL_VERSION {
            return Err(format!("unsupported media-ingress protocol version {}", self.version));
        }

        if self.len > MEDIA_INGRESS_MAX_PAYLOAD_BYTES {
            return Err(format!(
                "media-ingress payload exceeds {} GiB transport limit",
                MEDIA_INGRESS_MAX_PAYLOAD_BYTES / (1024 * 1024 * 1024)
            ));
        }

        match self.status {
            MediaIngressStatus::Ok => {
                if self.len == 0 {
                    return Err("media-ingress success response has an empty payload".to_string());
                }

                if self.name.as_deref().map(str::trim).filter(|name| !name.is_empty()).is_none() {
                    return Err("media-ingress success response has no file name".to_string());
                }
            }
            MediaIngressStatus::Cancel => {
                if self.len != 0 {
                    return Err("media-ingress cancel response carries a payload".to_string());
                }
            }
            MediaIngressStatus::Error => {
                if self.len != 0 {
                    return Err("media-ingress error response carries a payload".to_string());
                }
            }
        }

        Ok(())
    }

    pub fn encode_line(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut encoded = serde_json::to_vec(self)?;
        encoded.push(b'\n');
        Ok(encoded)
    }

    pub fn decode_line(line: &[u8]) -> Result<Self, String> {
        if line.len() > MEDIA_INGRESS_MAX_HEADER_BYTES {
            return Err("media-ingress header exceeds limit".to_string());
        }

        let header: Self = serde_json::from_slice(line)
            .map_err(|error| format!("invalid media-ingress header: {error}"))?;

        header.validate()?;
        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MediaIngressResponseHeader, MediaIngressStatus, MEDIA_INGRESS_MAX_PAYLOAD_BYTES,
        MEDIA_INGRESS_PROTOCOL_VERSION, MEDIA_INGRESS_REQUEST_LINE,
    };

    #[test]
    fn success_header_round_trips() {
        let header = MediaIngressResponseHeader::ok("photo.webp", 1234);

        let encoded = header.encode_line().expect("encode header");
        let decoded = MediaIngressResponseHeader::decode_line(&encoded).expect("decode header");

        assert_eq!(decoded, header);
        assert_eq!(decoded.status, MediaIngressStatus::Ok);
    }

    #[test]
    fn cancel_header_round_trips() {
        let header = MediaIngressResponseHeader::cancel();

        let encoded = header.encode_line().expect("encode header");
        let decoded = MediaIngressResponseHeader::decode_line(&encoded).expect("decode header");

        assert_eq!(decoded.status, MediaIngressStatus::Cancel);
        assert_eq!(decoded.len, 0);
    }

    #[test]
    fn request_is_media_generic_v2() {
        assert_eq!(MEDIA_INGRESS_PROTOCOL_VERSION, 2);
        assert!(MEDIA_INGRESS_REQUEST_LINE.contains("\"pick_media\""));
    }

    #[test]
    fn payload_transport_is_bounded_for_large_media() {
        let header =
            MediaIngressResponseHeader::ok("large.mkv", MEDIA_INGRESS_MAX_PAYLOAD_BYTES + 1);

        let error = header.validate().expect_err("must reject");

        assert!(error.contains("exceeds"));
        assert!(error.contains("GiB"));
    }

    #[test]
    fn success_requires_a_nonempty_name() {
        let header = MediaIngressResponseHeader {
            version: MEDIA_INGRESS_PROTOCOL_VERSION,
            status: MediaIngressStatus::Ok,
            name: Some(String::new()),
            len: 1,
            message: None,
        };

        assert!(header.validate().is_err());
    }
}
