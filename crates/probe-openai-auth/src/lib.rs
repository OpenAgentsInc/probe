use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use rand::rngs::OsRng;
use reqwest::StatusCode;
use reqwest::Url;
use reqwest::blocking::{Client, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DEFAULT_OPENAI_AUTH_ISSUER: &str = "https://auth.openai.com";
pub const DEFAULT_OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const DEFAULT_OPENAI_BROWSER_OAUTH_PORT: u16 = 1455;
pub const DEFAULT_POLLING_SAFETY_MARGIN_MS: u64 = 3_000;

const DEFAULT_BROWSER_TIMEOUT_SECS: u64 = 300;
const AUTH_FILE_RELATIVE_PATH: &str = "auth/openai-codex.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCodexAuthRecord {
    pub refresh: String,
    pub access: String,
    pub expires: u64,
    pub account_id: Option<String>,
}

impl OpenAiCodexAuthRecord {
    #[must_use]
    pub fn is_expired_at(&self, now_millis: u64) -> bool {
        self.expires <= now_millis
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexAuthStatus {
    pub path: PathBuf,
    pub authenticated: bool,
    pub expires: Option<u64>,
    pub expired: bool,
    pub account_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserLoginPrompt {
    pub authorize_url: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceLoginPrompt {
    pub verification_url: String,
    pub user_code: String,
    pub interval_ms: u64,
    device_auth_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiAuthConfig {
    pub issuer: String,
    pub client_id: String,
    pub oauth_port: u16,
    pub browser_timeout: Duration,
    pub polling_safety_margin: Duration,
}

impl Default for OpenAiAuthConfig {
    fn default() -> Self {
        Self {
            issuer: String::from(DEFAULT_OPENAI_AUTH_ISSUER),
            client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
            oauth_port: DEFAULT_OPENAI_BROWSER_OAUTH_PORT,
            browser_timeout: Duration::from_secs(DEFAULT_BROWSER_TIMEOUT_SECS),
            polling_safety_margin: Duration::from_millis(DEFAULT_POLLING_SAFETY_MARGIN_MS),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiCodexAuthStore {
    path: PathBuf,
}

impl OpenAiCodexAuthStore {
    #[must_use]
    pub fn new(probe_home: impl AsRef<Path>) -> Self {
        Self {
            path: probe_home.as_ref().join(AUTH_FILE_RELATIVE_PATH),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn load(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(self.path.as_path()).map_err(OpenAiAuthError::Io)?;
        let record = serde_json::from_str(&raw).map_err(OpenAiAuthError::Json)?;
        Ok(Some(record))
    }

    pub fn save(&self, record: &OpenAiCodexAuthRecord) -> Result<(), OpenAiAuthError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| OpenAiAuthError::Store(String::from("auth file must have a parent")))?;
        fs::create_dir_all(parent).map_err(OpenAiAuthError::Io)?;
        write_private_file(
            self.path.as_path(),
            serde_json::to_vec_pretty(record).map_err(OpenAiAuthError::Json)?,
        )
    }

    pub fn clear(&self) -> Result<bool, OpenAiAuthError> {
        match fs::remove_file(self.path.as_path()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(OpenAiAuthError::Io(error)),
        }
    }

    pub fn status(&self) -> Result<OpenAiCodexAuthStatus, OpenAiAuthError> {
        let record = self.load()?;
        let now_millis = unix_time_millis()?;
        Ok(match record {
            Some(record) => OpenAiCodexAuthStatus {
                path: self.path.clone(),
                authenticated: true,
                expires: Some(record.expires),
                expired: record.is_expired_at(now_millis),
                account_id: record.account_id,
            },
            None => OpenAiCodexAuthStatus {
                path: self.path.clone(),
                authenticated: false,
                expires: None,
                expired: false,
                account_id: None,
            },
        })
    }
}

#[derive(Clone, Debug)]
pub struct OpenAiCodexAuthController {
    client: Client,
    config: OpenAiAuthConfig,
    store: OpenAiCodexAuthStore,
}

impl OpenAiCodexAuthController {
    pub fn new(probe_home: impl AsRef<Path>) -> Result<Self, OpenAiAuthError> {
        Self::with_config(probe_home, OpenAiAuthConfig::default())
    }

    pub fn with_config(
        probe_home: impl AsRef<Path>,
        config: OpenAiAuthConfig,
    ) -> Result<Self, OpenAiAuthError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(OpenAiAuthError::Http)?;
        Ok(Self {
            client,
            config,
            store: OpenAiCodexAuthStore::new(probe_home),
        })
    }

    #[must_use]
    pub fn store(&self) -> &OpenAiCodexAuthStore {
        &self.store
    }

    pub fn status(&self) -> Result<OpenAiCodexAuthStatus, OpenAiAuthError> {
        self.store.status()
    }

    pub fn load(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        self.store.load()
    }

    pub fn clear(&self) -> Result<bool, OpenAiAuthError> {
        self.store.clear()
    }

    pub fn login_browser(
        &self,
        on_prompt: impl FnOnce(&BrowserLoginPrompt),
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let listener = bind_browser_callback_listener(self.config.oauth_port)?;
        let port = listener.local_addr().map_err(OpenAiAuthError::Io)?.port();
        let redirect_uri = format!("http://localhost:{port}/auth/callback");
        let pkce = generate_pkce();
        let state = generate_state();
        let prompt = BrowserLoginPrompt {
            authorize_url: build_authorize_url(
                self.config.issuer.as_str(),
                self.config.client_id.as_str(),
                redirect_uri.as_str(),
                pkce.challenge.as_str(),
                state.as_str(),
            )?,
            redirect_uri: redirect_uri.clone(),
        };
        on_prompt(&prompt);
        let code = wait_for_browser_callback(
            &listener,
            redirect_uri.as_str(),
            state.as_str(),
            self.config.browser_timeout,
        )?;
        let tokens = self.exchange_authorization_code(
            code.as_str(),
            redirect_uri.as_str(),
            pkce.verifier.as_str(),
        )?;
        self.persist_tokens(tokens, None)
    }

    pub fn login_device(
        &self,
        on_prompt: impl FnOnce(&DeviceLoginPrompt),
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let prompt = self.begin_device_authorization()?;
        on_prompt(&prompt);
        let tokens = self.poll_device_authorization(&prompt)?;
        self.persist_tokens(tokens, None)
    }

    pub fn refresh_now(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        let Some(record) = self.store.load()? else {
            return Ok(None);
        };
        let tokens = self.refresh_tokens(record.refresh.as_str())?;
        self.persist_tokens(tokens, record.account_id).map(Some)
    }

    pub fn refresh_if_needed(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        let Some(record) = self.store.load()? else {
            return Ok(None);
        };
        if !record.is_expired_at(unix_time_millis()?) {
            return Ok(Some(record));
        }
        let tokens = self.refresh_tokens(record.refresh.as_str())?;
        self.persist_tokens(tokens, record.account_id).map(Some)
    }

    fn begin_device_authorization(&self) -> Result<DeviceLoginPrompt, OpenAiAuthError> {
        let response = self
            .client
            .post(join_url(
                self.config.issuer.as_str(),
                "/api/accounts/deviceauth/usercode",
            )?)
            .json(&serde_json::json!({
                "client_id": self.config.client_id,
            }))
            .send()
            .map_err(OpenAiAuthError::Http)?;
        let payload: DeviceAuthorizationResponse = decode_json_response(response)?;
        let interval_ms = payload.interval_ms();
        Ok(DeviceLoginPrompt {
            verification_url: join_url(self.config.issuer.as_str(), "/codex/device")?,
            user_code: payload.user_code,
            interval_ms,
            device_auth_id: payload.device_auth_id,
        })
    }

    fn poll_device_authorization(
        &self,
        prompt: &DeviceLoginPrompt,
    ) -> Result<TokenResponse, OpenAiAuthError> {
        loop {
            let response = self
                .client
                .post(join_url(
                    self.config.issuer.as_str(),
                    "/api/accounts/deviceauth/token",
                )?)
                .json(&serde_json::json!({
                    "device_auth_id": prompt.device_auth_id,
                    "user_code": prompt.user_code,
                }))
                .send()
                .map_err(OpenAiAuthError::Http)?;
            if response.status().is_success() {
                let payload: DeviceAuthorizationCodeResponse = decode_json_response(response)?;
                return self.exchange_authorization_code(
                    payload.authorization_code.as_str(),
                    join_url(self.config.issuer.as_str(), "/deviceauth/callback")?.as_str(),
                    payload.code_verifier.as_str(),
                );
            }
            if !matches!(
                response.status(),
                StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST
            ) {
                let status = response.status();
                let body = response.text().map_err(OpenAiAuthError::Http)?;
                return Err(OpenAiAuthError::Protocol(format!(
                    "device authorization failed with http {status}: {body}"
                )));
            }
            std::thread::sleep(prompt.interval() + self.config.polling_safety_margin);
        }
    }

    fn exchange_authorization_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<TokenResponse, OpenAiAuthError> {
        let response = self
            .client
            .post(join_url(self.config.issuer.as_str(), "/oauth/token")?)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("client_id", self.config.client_id.as_str()),
                ("code_verifier", code_verifier),
            ])
            .send()
            .map_err(OpenAiAuthError::Http)?;
        decode_json_response(response)
    }

    fn refresh_tokens(&self, refresh_token: &str) -> Result<TokenResponse, OpenAiAuthError> {
        let response = self
            .client
            .post(join_url(self.config.issuer.as_str(), "/oauth/token")?)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", self.config.client_id.as_str()),
            ])
            .send()
            .map_err(OpenAiAuthError::Http)?;
        decode_json_response(response)
    }

    fn persist_tokens(
        &self,
        tokens: TokenResponse,
        fallback_account_id: Option<String>,
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let account_id = extract_account_id(&tokens).or(fallback_account_id);
        let expires = unix_time_millis()?
            .saturating_add(u64::from(tokens.expires_in.unwrap_or(3_600)).saturating_mul(1_000));
        let record = OpenAiCodexAuthRecord {
            refresh: tokens.refresh_token,
            access: tokens.access_token,
            expires,
            account_id,
        };
        self.store.save(&record)?;
        Ok(record)
    }
}

#[derive(Debug)]
pub enum OpenAiAuthError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Store(String),
    Protocol(String),
    BrowserTimeout { redirect_uri: String },
}

impl Display for OpenAiAuthError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Store(message) | Self::Protocol(message) => f.write_str(message),
            Self::BrowserTimeout { redirect_uri } => write!(
                f,
                "timed out waiting for the browser callback on {redirect_uri}"
            ),
        }
    }
}

impl std::error::Error for OpenAiAuthError {}

#[derive(Clone, Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    device_auth_id: String,
    user_code: String,
    #[serde(default)]
    interval: serde_json::Value,
}

impl DeviceAuthorizationResponse {
    fn interval_ms(&self) -> u64 {
        let raw = match &self.interval {
            serde_json::Value::Number(number) => number.as_u64().unwrap_or(5),
            serde_json::Value::String(value) => value.parse::<u64>().unwrap_or(5),
            _ => 5,
        };
        std::cmp::max(raw.saturating_mul(1_000), 1_000)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct DeviceAuthorizationCodeResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Clone, Debug, Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: String,
    refresh_token: String,
    expires_in: Option<u32>,
}

#[derive(Clone, Debug)]
struct PkceCodes {
    verifier: String,
    challenge: String,
}

impl DeviceLoginPrompt {
    fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }
}

fn build_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> Result<String, OpenAiAuthError> {
    let mut url = Url::parse(&join_url(issuer, "/oauth/authorize")?)
        .map_err(|error| OpenAiAuthError::Protocol(error.to_string()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "openid profile email offline_access")
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state)
        .append_pair("originator", "probe");
    Ok(url.to_string())
}

fn bind_browser_callback_listener(port: u16) -> Result<TcpListener, OpenAiAuthError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(OpenAiAuthError::Io)?;
    listener
        .set_nonblocking(true)
        .map_err(OpenAiAuthError::Io)?;
    Ok(listener)
}

fn wait_for_browser_callback(
    listener: &TcpListener,
    redirect_uri: &str,
    expected_state: &str,
    timeout: Duration,
) -> Result<String, OpenAiAuthError> {
    let started = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .map_err(OpenAiAuthError::Io)?;
                let request = read_request(&mut stream)?;
                let request_line = request.lines().next().ok_or_else(|| {
                    OpenAiAuthError::Protocol(String::from("missing callback request line"))
                })?;
                let path = request_line.split_whitespace().nth(1).ok_or_else(|| {
                    OpenAiAuthError::Protocol(String::from("missing callback path"))
                })?;
                let url = Url::parse(&format!("http://localhost{path}"))
                    .map_err(|error| OpenAiAuthError::Protocol(error.to_string()))?;
                let outcome = validate_browser_callback(&url, expected_state);
                let (status_code, body) = match &outcome {
                    Ok(_) => (200, html_success()),
                    Err(error) => (400, html_error(error.to_string().as_str())),
                };
                write_http_response(&mut stream, status_code, body.as_str())?;
                return outcome;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() >= timeout {
                    return Err(OpenAiAuthError::BrowserTimeout {
                        redirect_uri: String::from(redirect_uri),
                    });
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(OpenAiAuthError::Io(error)),
        }
    }
}

fn validate_browser_callback(url: &Url, expected_state: &str) -> Result<String, OpenAiAuthError> {
    if url.path() != "/auth/callback" {
        return Err(OpenAiAuthError::Protocol(String::from(
            "unexpected callback path",
        )));
    }
    let error = url
        .query_pairs()
        .find_map(|(key, value)| (key == "error").then(|| value.into_owned()));
    if let Some(error) = error {
        return Err(OpenAiAuthError::Protocol(error));
    }
    let state = url
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        .ok_or_else(|| OpenAiAuthError::Protocol(String::from("missing OAuth state")))?;
    if state != expected_state {
        return Err(OpenAiAuthError::Protocol(String::from(
            "OAuth callback state mismatch",
        )));
    }
    let code = url
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .ok_or_else(|| OpenAiAuthError::Protocol(String::from("missing authorization code")))?;
    Ok(code)
}

fn write_http_response(
    stream: &mut std::net::TcpStream,
    status_code: u16,
    body: &str,
) -> Result<(), OpenAiAuthError> {
    let payload = format!(
        "HTTP/1.1 {status_code} OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    stream
        .write_all(payload.as_bytes())
        .map_err(OpenAiAuthError::Io)
}

fn read_request(stream: &mut std::net::TcpStream) -> Result<String, OpenAiAuthError> {
    let mut request = String::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes) => {
                request.push_str(&String::from_utf8_lossy(&buffer[..bytes]));
                if bytes < buffer.len() {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(error) => return Err(OpenAiAuthError::Io(error)),
        }
    }
    Ok(request)
}

fn generate_pkce() -> PkceCodes {
    let verifier = random_url_safe_string(43);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    PkceCodes {
        verifier,
        challenge,
    }
}

fn generate_state() -> String {
    random_url_safe_string(32)
}

fn random_url_safe_string(length: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut bytes = vec![0_u8; length];
    OsRng.fill_bytes(bytes.as_mut_slice());
    bytes
        .into_iter()
        .map(|byte| CHARS[(byte as usize) % CHARS.len()] as char)
        .collect()
}

fn extract_account_id(tokens: &TokenResponse) -> Option<String> {
    tokens
        .id_token
        .as_deref()
        .and_then(parse_account_id_from_jwt)
        .or_else(|| parse_account_id_from_jwt(tokens.access_token.as_str()))
}

fn parse_account_id_from_jwt(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("chatgpt_account_id")
        .and_then(serde_json::Value::as_str)
        .map(String::from)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|value| value.get("chatgpt_account_id"))
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(serde_json::Value::as_array)
                .and_then(|values| values.first())
                .and_then(|value| value.get("id"))
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
}

fn decode_json_response<T: for<'de> Deserialize<'de>>(
    response: Response,
) -> Result<T, OpenAiAuthError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().map_err(OpenAiAuthError::Http)?;
        return Err(OpenAiAuthError::Protocol(format!(
            "backend returned http {status}: {body}"
        )));
    }
    response.json().map_err(OpenAiAuthError::Http)
}

fn join_url(base: &str, path: &str) -> Result<String, OpenAiAuthError> {
    let base = Url::parse(base).map_err(|error| OpenAiAuthError::Protocol(error.to_string()))?;
    let joined = base
        .join(path.trim_start_matches('/'))
        .map_err(|error| OpenAiAuthError::Protocol(error.to_string()))?;
    Ok(joined.to_string())
}

fn unix_time_millis() -> Result<u64, OpenAiAuthError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OpenAiAuthError::Store(error.to_string()))?;
    u64::try_from(now.as_millis())
        .map_err(|_| OpenAiAuthError::Store(String::from("unix timestamp exceeded u64")))
}

fn html_success() -> String {
    String::from(
        "<!doctype html><html><body><h1>Probe authorization complete</h1><p>You can close this window and return to Probe.</p></body></html>",
    )
}

fn html_error(message: &str) -> String {
    format!(
        "<!doctype html><html><body><h1>Probe authorization failed</h1><p>{message}</p></body></html>"
    )
}

fn write_private_file(path: &Path, bytes: Vec<u8>) -> Result<(), OpenAiAuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(OpenAiAuthError::Io)?;
        file.write_all(bytes.as_slice())
            .map_err(OpenAiAuthError::Io)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(OpenAiAuthError::Io)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        fs::write(path, bytes).map_err(OpenAiAuthError::Io)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use probe_test_support::{FakeAppleFmServer, FakeHttpRequest, FakeHttpResponse};
    use reqwest::Url;
    use tempfile::tempdir;

    use super::{
        DEFAULT_OPENAI_CLIENT_ID, OpenAiAuthConfig, OpenAiCodexAuthController,
        OpenAiCodexAuthRecord, OpenAiCodexAuthStore, build_authorize_url, extract_account_id,
        html_error, html_success,
    };

    #[test]
    fn store_round_trips_and_uses_private_permissions() {
        let temp = tempdir().expect("temp dir");
        let store = OpenAiCodexAuthStore::new(temp.path());
        let record = OpenAiCodexAuthRecord {
            refresh: String::from("refresh-token"),
            access: String::from("access-token"),
            expires: 1234,
            account_id: Some(String::from("acct-123")),
        };
        store.save(&record).expect("save auth state");
        let loaded = store
            .load()
            .expect("load auth state")
            .expect("auth record should exist");
        assert_eq!(loaded, record);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(store.path())
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn browser_login_persists_callback_tokens() {
        let temp = tempdir().expect("temp dir");
        let auth_server = FakeAppleFmServer::from_handler(|request: FakeHttpRequest| {
            assert_eq!(request.path, "/oauth/token");
            FakeHttpResponse::json_ok(serde_json::json!({
                "access_token": signed_token_with_account("acct-browser"),
                "refresh_token": "refresh-browser",
                "id_token": signed_token_with_account("acct-browser"),
                "expires_in": 3600
            }))
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: auth_server.base_url().to_string(),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(5),
                polling_safety_margin: Duration::from_millis(1),
            },
        )
        .expect("controller");

        let record = controller
            .login_browser(|prompt| {
                assert!(prompt.authorize_url.contains("/oauth/authorize?"));
                assert!(prompt.authorize_url.contains("originator=probe"));
                let authorize_url = prompt.authorize_url.clone();
                let redirect_uri = prompt.redirect_uri.clone();
                thread::spawn(move || {
                    let state = Url::parse(authorize_url.as_str())
                        .expect("authorize url")
                        .query_pairs()
                        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
                        .expect("oauth state");
                    send_browser_callback(
                        format!("{redirect_uri}?code=browser-code&state={state}").as_str(),
                    );
                });
            })
            .expect("browser login");

        assert_eq!(record.refresh, "refresh-browser");
        assert_eq!(record.account_id.as_deref(), Some("acct-browser"));
        assert!(
            controller
                .store()
                .load()
                .expect("load auth state")
                .is_some()
        );
    }

    #[test]
    fn device_login_polls_until_authorization_code_is_ready() {
        let temp = tempdir().expect("temp dir");
        let poll_count = Arc::new(Mutex::new(0_u32));
        let poll_counter = Arc::clone(&poll_count);
        let auth_server = FakeAppleFmServer::from_handler(move |request: FakeHttpRequest| {
            match request.path.as_str() {
                "/api/accounts/deviceauth/usercode" => {
                    FakeHttpResponse::json_ok(serde_json::json!({
                        "device_auth_id": "device-123",
                        "user_code": "ABC-123",
                        "interval": "0"
                    }))
                }
                "/api/accounts/deviceauth/token" => {
                    let mut guard = poll_counter.lock().expect("poll counter");
                    *guard += 1;
                    if *guard == 1 {
                        FakeHttpResponse::text_status(403, "pending")
                    } else {
                        FakeHttpResponse::json_ok(serde_json::json!({
                            "authorization_code": "device-code",
                            "code_verifier": "device-verifier"
                        }))
                    }
                }
                "/oauth/token" => FakeHttpResponse::json_ok(serde_json::json!({
                    "access_token": signed_token_with_account("acct-device"),
                    "refresh_token": "refresh-device",
                    "id_token": signed_token_with_account("acct-device"),
                    "expires_in": 1800
                })),
                other => panic!("unexpected request path: {other}"),
            }
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: auth_server.base_url().to_string(),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(2),
                polling_safety_margin: Duration::from_millis(1),
            },
        )
        .expect("controller");

        let record = controller
            .login_device(|prompt| {
                assert_eq!(prompt.user_code, "ABC-123");
                assert_eq!(
                    prompt.verification_url,
                    format!("{}/codex/device", auth_server.base_url())
                );
            })
            .expect("device login");

        assert_eq!(record.account_id.as_deref(), Some("acct-device"));
        assert_eq!(*poll_count.lock().expect("final poll count"), 2);
    }

    #[test]
    fn refresh_if_needed_rewrites_expired_auth_state() {
        let temp = tempdir().expect("temp dir");
        let auth_server = FakeAppleFmServer::from_handler(|request: FakeHttpRequest| {
            assert_eq!(request.path, "/oauth/token");
            FakeHttpResponse::json_ok(serde_json::json!({
                "access_token": signed_token_with_account("acct-refresh"),
                "refresh_token": "refresh-new",
                "id_token": signed_token_with_account("acct-refresh"),
                "expires_in": 7200
            }))
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: auth_server.base_url().to_string(),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(2),
                polling_safety_margin: Duration::from_millis(1),
            },
        )
        .expect("controller");
        controller
            .store()
            .save(&OpenAiCodexAuthRecord {
                refresh: String::from("refresh-old"),
                access: String::from("access-old"),
                expires: 1,
                account_id: Some(String::from("acct-old")),
            })
            .expect("seed auth");

        let refreshed = controller
            .refresh_if_needed()
            .expect("refresh result")
            .expect("record should exist");
        assert_eq!(refreshed.refresh, "refresh-new");
        assert_eq!(refreshed.account_id.as_deref(), Some("acct-refresh"));
        assert!(refreshed.expires > 1);
    }

    #[test]
    fn extract_account_id_supports_nested_and_org_claims() {
        let token_response: super::TokenResponse = serde_json::from_value(serde_json::json!({
            "id_token": signed_token(serde_json::json!({
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "acct-nested"
                }
            })),
            "access_token": signed_token(serde_json::json!({
                "organizations": [
                    { "id": "org-123" }
                ]
            })),
            "refresh_token": "refresh-token"
        }))
        .expect("decode token response");
        assert_eq!(
            extract_account_id(&token_response).as_deref(),
            Some("acct-nested")
        );
    }

    #[test]
    fn authorize_url_contains_codex_native_app_parameters() {
        let url = build_authorize_url(
            "https://auth.openai.com",
            DEFAULT_OPENAI_CLIENT_ID,
            "http://localhost:1455/auth/callback",
            "challenge",
            "state",
        )
        .expect("authorize url");
        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("originator=probe"));
    }

    #[test]
    fn callback_html_helpers_are_non_empty() {
        assert!(html_success().contains("Probe authorization complete"));
        assert!(html_error("oops").contains("oops"));
    }

    fn send_browser_callback(url: &str) {
        let parsed = Url::parse(url).expect("callback url");
        let host = parsed.host_str().expect("host");
        let port = parsed.port().expect("port");
        let mut path = parsed.path().to_string();
        if let Some(query) = parsed.query() {
            path.push('?');
            path.push_str(query);
        }
        let mut stream = TcpStream::connect((host, port)).expect("connect callback listener");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .expect("write callback");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read callback");
        assert!(response.contains("200 OK"));
    }

    fn signed_token_with_account(account_id: &str) -> String {
        signed_token(serde_json::json!({
            "chatgpt_account_id": account_id
        }))
    }

    fn signed_token(payload: serde_json::Value) -> String {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    }
}
