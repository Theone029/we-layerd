use serde::{Deserialize, Serialize};

pub const STILL_INGRESS_PROTOCOL_VERSION: u32 = 1;
pub const STILL_INGRESS_MAX_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024;
pub const STILL_INGRESS_MAX_HEADER_BYTES: usize = 8 * 1024;

pub const STILL_INGRESS_REQUEST_LINE: &str = "{\"version\":1,\"operation\":\"pick_still\"}\n";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StillIngressStatus {
    Ok,
    Cancel,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StillIngressResponseHeader {
    pub version: u32,
    pub status: StillIngressStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub len: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl StillIngressResponseHeader {
    pub fn ok(name: impl Into<String>, len: u64) -> Self {
        Self {
            version: STILL_INGRESS_PROTOCOL_VERSION,
            status: StillIngressStatus::Ok,
            name: Some(name.into()),
            len,
            message: None,
        }
    }

    pub fn cancel() -> Self {
        Self {
            version: STILL_INGRESS_PROTOCOL_VERSION,
            status: StillIngressStatus::Cancel,
            name: None,
            len: 0,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            version: STILL_INGRESS_PROTOCOL_VERSION,
            status: StillIngressStatus::Error,
            name: None,
            len: 0,
            message: Some(message.into()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != STILL_INGRESS_PROTOCOL_VERSION {
            return Err(format!("unsupported still-ingress protocol version {}", self.version));
        }

        if self.len > STILL_INGRESS_MAX_PAYLOAD_BYTES {
            return Err(format!(
                "still-ingress payload exceeds {} MiB limit",
                STILL_INGRESS_MAX_PAYLOAD_BYTES / (1024 * 1024)
            ));
        }

        match self.status {
            StillIngressStatus::Ok => {
                if self.len == 0 {
                    return Err("still-ingress success response has an empty payload".to_string());
                }

                if self.name.as_deref().map(str::trim).filter(|name| !name.is_empty()).is_none() {
                    return Err("still-ingress success response has no file name".to_string());
                }
            }
            StillIngressStatus::Cancel => {
                if self.len != 0 {
                    return Err("still-ingress cancel response carries a payload".to_string());
                }
            }
            StillIngressStatus::Error => {
                if self.len != 0 {
                    return Err("still-ingress error response carries a payload".to_string());
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
        if line.len() > STILL_INGRESS_MAX_HEADER_BYTES {
            return Err("still-ingress header exceeds limit".to_string());
        }

        let header: Self = serde_json::from_slice(line)
            .map_err(|error| format!("invalid still-ingress header: {error}"))?;

        header.validate()?;
        Ok(header)
    }
}

#[cfg(test)]
mod tests {
    use super::{StillIngressResponseHeader, StillIngressStatus, STILL_INGRESS_MAX_PAYLOAD_BYTES};

    #[test]
    fn success_header_round_trips() {
        let header = StillIngressResponseHeader::ok("photo.png", 1234);

        let encoded = header.encode_line().expect("encode header");
        let decoded = StillIngressResponseHeader::decode_line(&encoded).expect("decode header");

        assert_eq!(decoded, header);
        assert_eq!(decoded.status, StillIngressStatus::Ok);
    }

    #[test]
    fn cancel_header_round_trips() {
        let header = StillIngressResponseHeader::cancel();

        let encoded = header.encode_line().expect("encode header");
        let decoded = StillIngressResponseHeader::decode_line(&encoded).expect("decode header");

        assert_eq!(decoded.status, StillIngressStatus::Cancel);
        assert_eq!(decoded.len, 0);
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let header =
            StillIngressResponseHeader::ok("large.png", STILL_INGRESS_MAX_PAYLOAD_BYTES + 1);

        let error = header.validate().expect_err("must reject");

        assert!(error.contains("exceeds"));
    }

    #[test]
    fn success_requires_a_nonempty_name() {
        let header = StillIngressResponseHeader {
            version: 1,
            status: StillIngressStatus::Ok,
            name: Some(String::new()),
            len: 1,
            message: None,
        };

        assert!(header.validate().is_err());
    }
}
