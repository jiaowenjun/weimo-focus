use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::types::{
    BootstrapView, CreateEventCommand, CreateEventResult, ErrorEnvelope, EventInput, EventPreview,
};

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("无法连接 tracker backend: {0}")]
    Unavailable(String),
    #[error("后端响应不符合约定: {0}")]
    Protocol(String),
    #[error("后端拒绝请求 ({code}): {message}")]
    Rejected {
        status: StatusCode,
        code: String,
        message: String,
        fields: BTreeMap<String, String>,
    },
}

impl ApiError {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Rejected { code, .. } => Some(code),
            _ => None,
        }
    }
}

pub trait TrackerApi {
    fn bootstrap(&self) -> Result<BootstrapView, ApiError>;
    fn preview(&self, input: &EventInput) -> Result<EventPreview, ApiError>;
    fn create(&self, command: &CreateEventCommand) -> Result<CreateEventResult, ApiError>;
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: String,
    client: Client,
}

impl ApiClient {
    pub fn new(base_url: &str) -> Result<Self, ApiError> {
        let base_url = base_url.trim_end_matches('/');
        let parsed = reqwest::Url::parse(base_url)
            .map_err(|error| ApiError::Protocol(format!("服务地址无效: {error}")))?;
        if parsed.scheme() != "http"
            || parsed.host_str() != Some("127.0.0.1")
            || parsed.path() != "/"
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(ApiError::Protocol(
                "服务地址必须是 http://127.0.0.1:PORT".into(),
            ));
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| ApiError::Protocol(error.to_string()))?;
        Ok(Self {
            base_url: base_url.to_owned(),
            client,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn decode<T: DeserializeOwned>(
        &self,
        response: Result<Response, reqwest::Error>,
    ) -> Result<T, ApiError> {
        let response = response.map_err(|error| ApiError::Unavailable(error.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| ApiError::Protocol(error.to_string()))?;
        if status.is_success() {
            serde_json::from_str(&body)
                .map_err(|error| ApiError::Protocol(format!("{error}; response body: {body}")))
        } else {
            let envelope: ErrorEnvelope = serde_json::from_str(&body).map_err(|error| {
                ApiError::Protocol(format!(
                    "HTTP {status} 未返回 error envelope: {error}; response body: {body}"
                ))
            })?;
            Err(ApiError::Rejected {
                status,
                code: envelope.error.code,
                message: envelope.error.message,
                fields: envelope.error.fields,
            })
        }
    }
}

impl TrackerApi for ApiClient {
    fn bootstrap(&self) -> Result<BootstrapView, ApiError> {
        self.decode(self.client.get(self.url("/api/v1/bootstrap")).send())
    }

    fn preview(&self, input: &EventInput) -> Result<EventPreview, ApiError> {
        self.decode(
            self.client
                .post(self.url("/api/v1/events/preview"))
                .json(input)
                .send(),
        )
    }

    fn create(&self, command: &CreateEventCommand) -> Result<CreateEventResult, ApiError> {
        self.decode(
            self.client
                .post(self.url("/api/v1/events"))
                .json(command)
                .send(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;
    use crate::types::{CreateAction, EventInput, SyncOutcomeStatus, SyncStatus};

    fn serve_once(status: &str, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let status = status.to_owned();
        let body = body.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            let header_end;
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                if let Some(position) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                    header_end = position + 4;
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            sender.send(String::from_utf8(request).unwrap()).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    fn input() -> EventInput {
        EventInput {
            calendar_name: "学习".into(),
            event_date: "2026-08-30".into(),
            start_time: "21:00".into(),
            end_time: "21:30".into(),
            title: "资料分析".into(),
            first_item: 205,
            last_item: 218,
        }
    }

    #[test]
    fn create_posts_json_and_decodes_saved_but_unsynced() {
        let body = serde_json::json!({
            "event": {
                "id": "event-id",
                "calendar_name": "学习",
                "event_date": "2026-08-30",
                "start_time": "21:00:00",
                "end_time": "21:30:00",
                "title": "资料分析",
                "first_item": 205,
                "last_item": 218,
                "quantity": 14,
                "sync_status": "pending",
                "sync_attempts": 1,
                "last_sync_error": {"code": "calendar_missing", "message": "missing"}
            },
            "save_status": "created",
            "sync": {
                "status": "failed",
                "error": {"code": "calendar_missing", "message": "missing"}
            }
        })
        .to_string();
        let (url, request) = serve_once("201 Created", &body);
        let client = ApiClient::new(&url).unwrap();
        let result = client
            .create(&CreateEventCommand {
                event: input(),
                action: CreateAction::SaveAndSync,
                allow_duplicate: false,
            })
            .unwrap();

        assert_eq!(result.event.sync_status, SyncStatus::Pending);
        assert_eq!(result.sync.status, SyncOutcomeStatus::Failed);
        let request = request.recv().unwrap();
        assert!(request.starts_with("POST /api/v1/events HTTP/1.1"));
        assert!(request.contains(r#""action":"save_and_sync""#));
        assert!(!request.contains("sync_marker"));
    }

    #[test]
    fn rejects_non_success_with_the_stable_error_envelope() {
        let body = serde_json::json!({
            "error": {
                "code": "duplicate_event",
                "message": "相同记录已经存在"
            }
        })
        .to_string();
        let (url, _request) = serve_once("409 Conflict", &body);
        let error = ApiClient::new(&url).unwrap().preview(&input()).unwrap_err();

        assert_eq!(error.code(), Some("duplicate_event"));
    }

    #[test]
    fn server_override_is_still_limited_to_loopback_http() {
        assert!(ApiClient::new("https://127.0.0.1:9123").is_err());
        assert!(ApiClient::new("http://example.com:9123").is_err());
        assert!(ApiClient::new("http://127.0.0.1:9123").is_ok());
    }
}
