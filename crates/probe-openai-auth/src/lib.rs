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
pub const DEFAULT_CHATGPT_BACKEND_API_BASE_URL: &str = "https://chatgpt.com/backend-api";

const DEFAULT_BROWSER_TIMEOUT_SECS: u64 = 300;
const DEFAULT_RATE_LIMIT_CACHE_TTL_MS: u64 = 60_000;
const AUTH_FILE_FORMAT_VERSION: u32 = 2;
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCodexRateLimitWindow {
    pub used_percent: u32,
    pub limit_window_seconds: u64,
    pub reset_after_seconds: u64,
    pub reset_at: u64,
}

impl OpenAiCodexRateLimitWindow {
    fn from_payload(raw: RawRateLimitWindowSnapshot) -> Option<Self> {
        Some(Self {
            used_percent: u32::try_from(raw.used_percent.max(0)).ok()?,
            limit_window_seconds: u64::try_from(raw.limit_window_seconds.max(0)).ok()?,
            reset_after_seconds: u64::try_from(raw.reset_after_seconds.max(0)).ok()?,
            reset_at: u64::try_from(raw.reset_at.max(0)).ok()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCodexRateLimitSnapshot {
    pub fetched_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    pub allowed: bool,
    pub limit_reached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_window: Option<OpenAiCodexRateLimitWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_window: Option<OpenAiCodexRateLimitWindow>,
}

impl OpenAiCodexRateLimitSnapshot {
    #[must_use]
    pub fn used_percent(&self) -> Option<u32> {
        self.primary_window
            .as_ref()
            .or(self.secondary_window.as_ref())
            .map(|window| window.used_percent)
    }

    #[must_use]
    pub fn reset_after_seconds(&self) -> Option<u64> {
        self.primary_window
            .as_ref()
            .or(self.secondary_window.as_ref())
            .map(|window| window.reset_after_seconds)
    }

    #[must_use]
    pub fn is_routable(&self) -> bool {
        self.allowed && !self.limit_reached
    }

    #[must_use]
    pub fn is_limited(&self) -> bool {
        !self.allowed || self.limit_reached
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiCodexAuthAccount {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub record: OpenAiCodexAuthRecord,
    pub added_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_selected_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<OpenAiCodexRateLimitSnapshot>,
}

impl OpenAiCodexAuthAccount {
    fn from_record(record: OpenAiCodexAuthRecord, label: Option<String>, now_millis: u64) -> Self {
        Self {
            key: account_storage_key(&record),
            label: normalize_account_label(label),
            record,
            added_at_ms: now_millis,
            last_selected_at_ms: Some(now_millis),
            rate_limits: None,
        }
    }

    #[must_use]
    pub fn is_expired_at(&self, now_millis: u64) -> bool {
        self.record.is_expired_at(now_millis)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct OpenAiCodexAuthState {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_account_key: Option<String>,
    #[serde(default)]
    pub accounts: Vec<OpenAiCodexAuthAccount>,
}

impl Default for OpenAiCodexAuthState {
    fn default() -> Self {
        Self {
            version: AUTH_FILE_FORMAT_VERSION,
            selected_account_key: None,
            accounts: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexAuthAccountStatus {
    pub key: String,
    pub label: Option<String>,
    pub user_email: Option<String>,
    pub expires: u64,
    pub expired: bool,
    pub account_id: Option<String>,
    pub selected: bool,
    pub rate_limits: Option<OpenAiCodexRateLimitSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexAuthStatus {
    pub path: PathBuf,
    pub authenticated: bool,
    pub expires: Option<u64>,
    pub expired: bool,
    pub account_id: Option<String>,
    pub selected_account_key: Option<String>,
    pub selected_account_label: Option<String>,
    pub selected_account_email: Option<String>,
    pub account_count: usize,
    pub accounts: Vec<OpenAiCodexAuthAccountStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexAccountRoute {
    pub account_key: String,
    pub label: Option<String>,
    pub record: OpenAiCodexAuthRecord,
    pub rate_limits: Option<OpenAiCodexRateLimitSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexApiKeyFallbackRoute {
    pub env_var: String,
    pub api_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenAiCodexRoute {
    SubscriptionAccount(OpenAiCodexAccountRoute),
    ApiKeyFallback(OpenAiCodexApiKeyFallbackRoute),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexRoutingPlan {
    pub routes: Vec<OpenAiCodexRoute>,
    pub api_key_fallback_available: bool,
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
    pub backend_api_base_url: String,
    pub rate_limit_cache_ttl: Duration,
}

impl Default for OpenAiAuthConfig {
    fn default() -> Self {
        Self {
            issuer: String::from(DEFAULT_OPENAI_AUTH_ISSUER),
            client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
            oauth_port: DEFAULT_OPENAI_BROWSER_OAUTH_PORT,
            browser_timeout: Duration::from_secs(DEFAULT_BROWSER_TIMEOUT_SECS),
            polling_safety_margin: Duration::from_millis(DEFAULT_POLLING_SAFETY_MARGIN_MS),
            backend_api_base_url: String::from(DEFAULT_CHATGPT_BACKEND_API_BASE_URL),
            rate_limit_cache_ttl: Duration::from_millis(DEFAULT_RATE_LIMIT_CACHE_TTL_MS),
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
        Ok(self
            .load_state()?
            .and_then(|state| selected_account(&state).map(|account| account.record.clone())))
    }

    pub fn save(&self, record: &OpenAiCodexAuthRecord) -> Result<(), OpenAiAuthError> {
        self.save_account(record, None).map(|_| ())
    }

    pub fn save_account(
        &self,
        record: &OpenAiCodexAuthRecord,
        label: Option<String>,
    ) -> Result<OpenAiCodexAuthAccount, OpenAiAuthError> {
        let now_millis = unix_time_millis()?;
        let mut state = self.load_state()?.unwrap_or_default();
        let mut account = OpenAiCodexAuthAccount::from_record(record.clone(), label, now_millis);
        if let Some(existing_index) = state.accounts.iter().position(|existing| {
            existing.key == account.key
                || (existing.record.account_id.is_some()
                    && existing.record.account_id == account.record.account_id)
        }) {
            let existing = &mut state.accounts[existing_index];
            account.added_at_ms = existing.added_at_ms;
            account.last_selected_at_ms = Some(now_millis);
            account.rate_limits = existing.rate_limits.clone();
            if account.label.is_none() {
                account.label = existing.label.clone();
            }
            *existing = account.clone();
        } else {
            state.accounts.push(account.clone());
        }
        state.version = AUTH_FILE_FORMAT_VERSION;
        state.selected_account_key = Some(account.key.clone());
        self.save_state(&state)?;
        Ok(account)
    }

    pub fn clear(&self) -> Result<bool, OpenAiAuthError> {
        match fs::remove_file(self.path.as_path()) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(OpenAiAuthError::Io(error)),
        }
    }

    pub fn clear_account(&self, selector: &str) -> Result<bool, OpenAiAuthError> {
        let Some(mut state) = self.load_state()? else {
            return Ok(false);
        };
        let original_len = state.accounts.len();
        state
            .accounts
            .retain(|account| !account_matches_selector(account, selector));
        if state.accounts.len() == original_len {
            return Ok(false);
        }
        if state.accounts.is_empty() {
            return self.clear();
        }
        if state.selected_account_key.as_ref().is_some_and(|selected| {
            !state
                .accounts
                .iter()
                .any(|account| account.key.as_str() == selected.as_str())
        }) {
            state.selected_account_key = state.accounts.first().map(|account| account.key.clone());
        }
        self.save_state(&state)?;
        Ok(true)
    }

    pub fn status(&self) -> Result<OpenAiCodexAuthStatus, OpenAiAuthError> {
        let state = self.load_state()?;
        let now_millis = unix_time_millis()?;
        Ok(match state {
            Some(state) => {
                let selected = selected_account(&state);
                let accounts = state
                    .accounts
                    .iter()
                    .map(|account| OpenAiCodexAuthAccountStatus {
                        key: account.key.clone(),
                        label: account.label.clone(),
                        user_email: parse_profile_email_from_jwt(account.record.access.as_str()),
                        expires: account.record.expires,
                        expired: account.is_expired_at(now_millis),
                        account_id: account.record.account_id.clone(),
                        selected: state
                            .selected_account_key
                            .as_ref()
                            .is_some_and(|selected| selected == &account.key),
                        rate_limits: account.rate_limits.clone(),
                    })
                    .collect::<Vec<_>>();
                OpenAiCodexAuthStatus {
                    path: self.path.clone(),
                    authenticated: !state.accounts.is_empty(),
                    expires: selected.map(|account| account.record.expires),
                    expired: selected.is_some_and(|account| account.is_expired_at(now_millis)),
                    account_id: selected.and_then(|account| account.record.account_id.clone()),
                    selected_account_key: state.selected_account_key.clone(),
                    selected_account_label: selected.and_then(|account| account.label.clone()),
                    selected_account_email: selected.and_then(|account| {
                        parse_profile_email_from_jwt(account.record.access.as_str())
                    }),
                    account_count: state.accounts.len(),
                    accounts,
                }
            }
            None => OpenAiCodexAuthStatus {
                path: self.path.clone(),
                authenticated: false,
                expires: None,
                expired: false,
                account_id: None,
                selected_account_key: None,
                selected_account_label: None,
                selected_account_email: None,
                account_count: 0,
                accounts: Vec::new(),
            },
        })
    }

    fn load_state(&self) -> Result<Option<OpenAiCodexAuthState>, OpenAiAuthError> {
        if !self.path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(self.path.as_path()).map_err(OpenAiAuthError::Io)?;
        let payload: AuthFilePayload = serde_json::from_str(&raw).map_err(OpenAiAuthError::Json)?;
        Ok(Some(match payload {
            AuthFilePayload::State(state) => normalize_auth_state(state),
            AuthFilePayload::Legacy(record) => {
                let now_millis = unix_time_millis()?;
                normalize_auth_state(OpenAiCodexAuthState {
                    version: AUTH_FILE_FORMAT_VERSION,
                    selected_account_key: Some(account_storage_key(&record)),
                    accounts: vec![OpenAiCodexAuthAccount::from_record(
                        record, None, now_millis,
                    )],
                })
            }
        }))
    }

    fn save_state(&self, state: &OpenAiCodexAuthState) -> Result<(), OpenAiAuthError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| OpenAiAuthError::Store(String::from("auth file must have a parent")))?;
        fs::create_dir_all(parent).map_err(OpenAiAuthError::Io)?;
        let normalized = normalize_auth_state(state.clone());
        write_private_file(
            self.path.as_path(),
            serde_json::to_vec_pretty(&normalized).map_err(OpenAiAuthError::Json)?,
        )
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

    pub fn refresh_accounts_for_routing(&self) -> Result<(), OpenAiAuthError> {
        let Some(mut state) = self.store.load_state()? else {
            return Ok(());
        };
        self.refresh_state_for_routing(&mut state)?;
        self.store.save_state(&state)
    }

    pub fn load(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        self.store.load()
    }

    pub fn clear(&self) -> Result<bool, OpenAiAuthError> {
        self.store.clear()
    }

    pub fn clear_account(&self, selector: &str) -> Result<bool, OpenAiAuthError> {
        self.store.clear_account(selector)
    }

    pub fn login_browser(
        &self,
        on_prompt: impl FnOnce(&BrowserLoginPrompt),
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        self.login_browser_with_label(None, on_prompt)
    }

    pub fn login_browser_with_label(
        &self,
        label: Option<String>,
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
        self.persist_tokens(tokens, None, label)
    }

    pub fn login_device(
        &self,
        on_prompt: impl FnOnce(&DeviceLoginPrompt),
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        self.login_device_with_label(None, on_prompt)
    }

    pub fn login_device_with_label(
        &self,
        label: Option<String>,
        on_prompt: impl FnOnce(&DeviceLoginPrompt),
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let prompt = self.begin_device_authorization()?;
        on_prompt(&prompt);
        let tokens = self.poll_device_authorization(&prompt)?;
        self.persist_tokens(tokens, None, label)
    }

    pub fn refresh_now(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        let Some(mut state) = self.store.load_state()? else {
            return Ok(None);
        };
        let Some(account) = selected_account_mut(&mut state) else {
            return Ok(None);
        };
        let record = self.refresh_account(account)?;
        self.store.save_state(&state)?;
        Ok(Some(record))
    }

    pub fn refresh_if_needed(&self) -> Result<Option<OpenAiCodexAuthRecord>, OpenAiAuthError> {
        let Some(mut state) = self.store.load_state()? else {
            return Ok(None);
        };
        let now_millis = unix_time_millis()?;
        let Some(account) = selected_account_mut(&mut state) else {
            return Ok(None);
        };
        if !account.is_expired_at(now_millis) {
            return Ok(Some(account.record.clone()));
        }
        let record = self.refresh_account(account)?;
        self.store.save_state(&state)?;
        Ok(Some(record))
    }

    pub fn routing_plan(
        &self,
        api_key_env: Option<&str>,
    ) -> Result<OpenAiCodexRoutingPlan, OpenAiAuthError> {
        let fallback = api_key_env.and_then(|env_var| {
            read_env_token(env_var).map(|api_key| {
                OpenAiCodexRoute::ApiKeyFallback(OpenAiCodexApiKeyFallbackRoute {
                    env_var: env_var.to_string(),
                    api_key,
                })
            })
        });
        let api_key_fallback_available = fallback.is_some();
        let Some(mut state) = self.store.load_state()? else {
            return Ok(OpenAiCodexRoutingPlan {
                routes: fallback.into_iter().collect(),
                api_key_fallback_available,
            });
        };
        self.refresh_state_for_routing(&mut state)?;
        self.store.save_state(&state)?;
        let now_millis = unix_time_millis()?;
        let selected_key = state.selected_account_key.clone();
        let mut routes = state
            .accounts
            .iter()
            .filter(|account| !account.is_expired_at(now_millis))
            .filter(|account| {
                account
                    .rate_limits
                    .as_ref()
                    .map(|snapshot| !snapshot.is_limited())
                    .unwrap_or(true)
            })
            .map(|account| {
                OpenAiCodexRoute::SubscriptionAccount(OpenAiCodexAccountRoute {
                    account_key: account.key.clone(),
                    label: account.label.clone(),
                    record: account.record.clone(),
                    rate_limits: account.rate_limits.clone(),
                })
            })
            .collect::<Vec<_>>();
        routes.sort_by(|left, right| compare_routes(left, right, selected_key.as_deref()));

        if routes.is_empty() {
            let has_limited_account = state.accounts.iter().any(|account| {
                !account.is_expired_at(now_millis)
                    && account
                        .rate_limits
                        .as_ref()
                        .is_some_and(OpenAiCodexRateLimitSnapshot::is_limited)
            });
            if has_limited_account {
                if let Some(fallback) = fallback {
                    routes.push(fallback);
                } else {
                    return Err(OpenAiAuthError::AllCodexAccountsRateLimited {
                        path: self.store.path().display().to_string(),
                        api_key_env: api_key_env.unwrap_or("PROBE_OPENAI_API_KEY").to_string(),
                    });
                }
            } else if !state.accounts.is_empty() {
                if let Some(fallback) = fallback {
                    routes.push(fallback);
                } else {
                    return Err(OpenAiAuthError::NoUsableCodexAccounts {
                        path: self.store.path().display().to_string(),
                    });
                }
            }
        } else if let Some(fallback) = fallback {
            routes.push(fallback);
        }

        Ok(OpenAiCodexRoutingPlan {
            routes,
            api_key_fallback_available,
        })
    }

    pub fn mark_selected_account(&self, account_key: &str) -> Result<(), OpenAiAuthError> {
        let Some(mut state) = self.store.load_state()? else {
            return Err(OpenAiAuthError::Store(format!(
                "codex auth state missing at {}",
                self.store.path().display()
            )));
        };
        if !state
            .accounts
            .iter()
            .any(|account| account.key == account_key)
        {
            return Err(OpenAiAuthError::Store(format!(
                "codex auth account `{account_key}` was not found in {}",
                self.store.path().display()
            )));
        }
        let now_millis = unix_time_millis()?;
        state.selected_account_key = Some(account_key.to_string());
        if let Some(account) = state
            .accounts
            .iter_mut()
            .find(|account| account.key == account_key)
        {
            account.last_selected_at_ms = Some(now_millis);
        }
        self.store.save_state(&state)
    }

    pub fn mark_account_rate_limited(
        &self,
        account_key: &str,
        body: &str,
    ) -> Result<(), OpenAiAuthError> {
        let Some(mut state) = self.store.load_state()? else {
            return Ok(());
        };
        let Some(account) = state
            .accounts
            .iter_mut()
            .find(|account| account.key == account_key)
        else {
            return Ok(());
        };
        let now_millis = unix_time_millis()?;
        let parsed = parse_rate_limit_error_body(body);
        let reset_after_seconds = parsed
            .as_ref()
            .and_then(|hint| hint.reset_after_seconds)
            .unwrap_or(0);
        let reset_at = parsed
            .as_ref()
            .and_then(|hint| hint.reset_at)
            .unwrap_or_else(|| now_millis / 1_000 + reset_after_seconds);
        account.rate_limits = Some(OpenAiCodexRateLimitSnapshot {
            fetched_at_ms: now_millis,
            plan_type: parsed
                .as_ref()
                .and_then(|hint| hint.plan_type.clone())
                .or_else(|| {
                    account
                        .rate_limits
                        .as_ref()
                        .and_then(|snapshot| snapshot.plan_type.clone())
                }),
            allowed: false,
            limit_reached: true,
            primary_window: Some(OpenAiCodexRateLimitWindow {
                used_percent: 100,
                limit_window_seconds: 0,
                reset_after_seconds,
                reset_at,
            }),
            secondary_window: None,
        });
        self.store.save_state(&state)
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
        label: Option<String>,
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let record = record_from_tokens(tokens, fallback_account_id)?;
        self.store.save_account(&record, label)?;
        Ok(record)
    }

    fn refresh_account(
        &self,
        account: &mut OpenAiCodexAuthAccount,
    ) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
        let tokens = self.refresh_tokens(account.record.refresh.as_str())?;
        let record = record_from_tokens(tokens, account.record.account_id.clone())?;
        account.record = record.clone();
        account.rate_limits = None;
        Ok(record)
    }

    fn refresh_state_for_routing(
        &self,
        state: &mut OpenAiCodexAuthState,
    ) -> Result<(), OpenAiAuthError> {
        let now_millis = unix_time_millis()?;
        for account in &mut state.accounts {
            if account.is_expired_at(now_millis) {
                match self.refresh_account(account) {
                    Ok(_) => {}
                    Err(_) => {
                        account.rate_limits = None;
                        continue;
                    }
                }
            }
            let snapshot_is_fresh = account.rate_limits.as_ref().is_some_and(|snapshot| {
                now_millis.saturating_sub(snapshot.fetched_at_ms)
                    < self.config.rate_limit_cache_ttl.as_millis() as u64
            });
            if snapshot_is_fresh {
                continue;
            }
            if let Ok(snapshot) = self.fetch_rate_limits(&account.record) {
                account.rate_limits = Some(snapshot);
            }
        }
        Ok(())
    }

    fn fetch_rate_limits(
        &self,
        record: &OpenAiCodexAuthRecord,
    ) -> Result<OpenAiCodexRateLimitSnapshot, OpenAiAuthError> {
        let mut request = self
            .client
            .get(join_url(
                self.config.backend_api_base_url.as_str(),
                "/wham/usage",
            )?)
            .bearer_auth(record.access.as_str())
            .header("originator", "probe")
            .header("User-Agent", "probe-auth");
        if let Some(account_id) = record.account_id.as_deref() {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request.send().map_err(OpenAiAuthError::Http)?;
        let payload: RawRateLimitStatusPayload = decode_json_response(response)?;
        let now_millis = unix_time_millis()?;
        Ok(OpenAiCodexRateLimitSnapshot {
            fetched_at_ms: now_millis,
            plan_type: payload.plan_type,
            allowed: payload
                .rate_limit
                .as_ref()
                .map(|details| details.allowed)
                .unwrap_or(true),
            limit_reached: payload
                .rate_limit
                .as_ref()
                .map(|details| details.limit_reached)
                .unwrap_or(false),
            primary_window: payload
                .rate_limit
                .as_ref()
                .and_then(|details| details.primary_window.clone())
                .and_then(OpenAiCodexRateLimitWindow::from_payload),
            secondary_window: payload
                .rate_limit
                .as_ref()
                .and_then(|details| details.secondary_window.clone())
                .and_then(OpenAiCodexRateLimitWindow::from_payload),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AuthFilePayload {
    State(OpenAiCodexAuthState),
    Legacy(OpenAiCodexAuthRecord),
}

#[derive(Clone, Debug, Deserialize)]
struct RawRateLimitStatusPayload {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RawRateLimitStatusDetails>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawRateLimitStatusDetails {
    allowed: bool,
    limit_reached: bool,
    #[serde(default)]
    primary_window: Option<RawRateLimitWindowSnapshot>,
    #[serde(default)]
    secondary_window: Option<RawRateLimitWindowSnapshot>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawRateLimitWindowSnapshot {
    used_percent: i32,
    limit_window_seconds: i32,
    reset_after_seconds: i32,
    reset_at: i32,
}

#[derive(Clone, Debug)]
struct ParsedRateLimitErrorBody {
    plan_type: Option<String>,
    reset_after_seconds: Option<u64>,
    reset_at: Option<u64>,
}

fn normalize_auth_state(mut state: OpenAiCodexAuthState) -> OpenAiCodexAuthState {
    state.version = AUTH_FILE_FORMAT_VERSION;
    if state.accounts.is_empty() {
        state.selected_account_key = None;
        return state;
    }
    let selected_is_valid = state.selected_account_key.as_ref().is_some_and(|selected| {
        state
            .accounts
            .iter()
            .any(|account| account.key.as_str() == selected.as_str())
    });
    if !selected_is_valid {
        state.selected_account_key = state.accounts.first().map(|account| account.key.clone());
    }
    state
}

fn selected_account(state: &OpenAiCodexAuthState) -> Option<&OpenAiCodexAuthAccount> {
    let selected_key = state.selected_account_key.as_deref()?;
    state
        .accounts
        .iter()
        .find(|account| account.key == selected_key)
}

fn selected_account_mut(state: &mut OpenAiCodexAuthState) -> Option<&mut OpenAiCodexAuthAccount> {
    let selected_key = state.selected_account_key.as_deref()?;
    state
        .accounts
        .iter_mut()
        .find(|account| account.key == selected_key)
}

fn account_storage_key(record: &OpenAiCodexAuthRecord) -> String {
    if let Some(account_id) = record
        .account_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return format!("acct:{account_id}");
    }
    let digest = Sha256::digest(record.refresh.as_bytes());
    let encoded = URL_SAFE_NO_PAD.encode(digest);
    format!("token:{}", &encoded[..16])
}

fn normalize_account_label(label: Option<String>) -> Option<String> {
    label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn account_matches_selector(account: &OpenAiCodexAuthAccount, selector: &str) -> bool {
    account.key == selector || account.record.account_id.as_deref() == Some(selector)
}

fn compare_routes(
    left: &OpenAiCodexRoute,
    right: &OpenAiCodexRoute,
    selected_key: Option<&str>,
) -> std::cmp::Ordering {
    match (left, right) {
        (
            OpenAiCodexRoute::SubscriptionAccount(left),
            OpenAiCodexRoute::SubscriptionAccount(right),
        ) => compare_subscription_routes(left, right, selected_key),
        (OpenAiCodexRoute::SubscriptionAccount(_), OpenAiCodexRoute::ApiKeyFallback(_)) => {
            std::cmp::Ordering::Less
        }
        (OpenAiCodexRoute::ApiKeyFallback(_), OpenAiCodexRoute::SubscriptionAccount(_)) => {
            std::cmp::Ordering::Greater
        }
        (OpenAiCodexRoute::ApiKeyFallback(_), OpenAiCodexRoute::ApiKeyFallback(_)) => {
            std::cmp::Ordering::Equal
        }
    }
}

fn compare_subscription_routes(
    left: &OpenAiCodexAccountRoute,
    right: &OpenAiCodexAccountRoute,
    selected_key: Option<&str>,
) -> std::cmp::Ordering {
    let left_group = route_priority_group(left);
    let right_group = route_priority_group(right);
    left_group
        .cmp(&right_group)
        .then_with(|| {
            let left_used = left
                .rate_limits
                .as_ref()
                .and_then(OpenAiCodexRateLimitSnapshot::used_percent)
                .unwrap_or(u32::MAX);
            let right_used = right
                .rate_limits
                .as_ref()
                .and_then(OpenAiCodexRateLimitSnapshot::used_percent)
                .unwrap_or(u32::MAX);
            left_used.cmp(&right_used)
        })
        .then_with(|| {
            let left_selected = Some(left.account_key.as_str()) == selected_key;
            let right_selected = Some(right.account_key.as_str()) == selected_key;
            right_selected.cmp(&left_selected)
        })
        .then_with(|| left.account_key.cmp(&right.account_key))
}

fn route_priority_group(route: &OpenAiCodexAccountRoute) -> u8 {
    match route.rate_limits.as_ref() {
        Some(snapshot) if snapshot.is_routable() => 0,
        None => 1,
        Some(_) => 2,
    }
}

fn read_env_token(env_var: &str) -> Option<String> {
    std::env::var(env_var)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn record_from_tokens(
    tokens: TokenResponse,
    fallback_account_id: Option<String>,
) -> Result<OpenAiCodexAuthRecord, OpenAiAuthError> {
    let account_id = extract_account_id(&tokens).or(fallback_account_id);
    let expires = unix_time_millis()?
        .saturating_add(u64::from(tokens.expires_in.unwrap_or(3_600)).saturating_mul(1_000));
    Ok(OpenAiCodexAuthRecord {
        refresh: tokens.refresh_token,
        access: tokens.access_token,
        expires,
        account_id,
    })
}

fn parse_rate_limit_error_body(body: &str) -> Option<ParsedRateLimitErrorBody> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = parsed.get("error").unwrap_or(&parsed);
    Some(ParsedRateLimitErrorBody {
        plan_type: error
            .get("plan_type")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        reset_after_seconds: error
            .get("resets_in_seconds")
            .or_else(|| error.get("reset_after_seconds"))
            .and_then(serde_json::Value::as_u64),
        reset_at: error
            .get("resets_at")
            .or_else(|| error.get("reset_at"))
            .and_then(serde_json::Value::as_u64),
    })
}

#[derive(Debug)]
pub enum OpenAiAuthError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Http(reqwest::Error),
    Store(String),
    Protocol(String),
    AllCodexAccountsRateLimited { path: String, api_key_env: String },
    NoUsableCodexAccounts { path: String },
    BrowserTimeout { redirect_uri: String },
}

impl Display for OpenAiAuthError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
            Self::Http(error) => write!(f, "http error: {error}"),
            Self::Store(message) | Self::Protocol(message) => f.write_str(message),
            Self::AllCodexAccountsRateLimited { path, api_key_env } => write!(
                f,
                "all saved Codex accounts at {path} are currently rate-limited; wait for reset or export `{api_key_env}` to use the optional API-key fallback"
            ),
            Self::NoUsableCodexAccounts { path } => write!(
                f,
                "saved Codex accounts at {path} are expired or unusable; re-run `probe codex login` for a working account"
            ),
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
    let claims = decode_jwt_claims(token)?;
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

fn parse_profile_email_from_jwt(token: &str) -> Option<String> {
    let claims = decode_jwt_claims(token)?;
    claims
        .get("email")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")
                .and_then(|value| value.get("email"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(String::from)
        })
}

fn decode_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    serde_json::from_slice(&decoded).ok()
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
        DEFAULT_OPENAI_CLIENT_ID, OpenAiAuthConfig, OpenAiAuthError, OpenAiCodexAuthController,
        OpenAiCodexAuthRecord, OpenAiCodexAuthStore, OpenAiCodexRoute, build_authorize_url,
        extract_account_id, html_error, html_success, unix_time_millis,
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
                backend_api_base_url: auth_server.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(60),
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
                backend_api_base_url: auth_server.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(60),
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
                backend_api_base_url: auth_server.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(60),
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
    fn legacy_single_record_file_migrates_into_multi_account_status() {
        let temp = tempdir().expect("temp dir");
        let store = OpenAiCodexAuthStore::new(temp.path());
        fs::create_dir_all(
            store
                .path()
                .parent()
                .expect("auth file should have parent directory"),
        )
        .expect("create auth dir");
        fs::write(
            store.path(),
            serde_json::to_vec_pretty(&OpenAiCodexAuthRecord {
                refresh: String::from("refresh-legacy"),
                access: String::from("access-legacy"),
                expires: 4_200,
                account_id: Some(String::from("acct-legacy")),
            })
            .expect("legacy auth json"),
        )
        .expect("write legacy auth json");

        let status = store.status().expect("status");
        assert!(status.authenticated);
        assert_eq!(status.account_count, 1);
        assert_eq!(status.account_id.as_deref(), Some("acct-legacy"));
        assert_eq!(
            status.selected_account_key.as_deref(),
            Some("acct:acct-legacy")
        );
    }

    #[test]
    fn status_derives_account_email_from_access_token() {
        let temp = tempdir().expect("temp dir");
        let store = OpenAiCodexAuthStore::new(temp.path());
        store
            .save(&OpenAiCodexAuthRecord {
                refresh: String::from("refresh-email"),
                access: signed_token(serde_json::json!({
                    "https://api.openai.com/profile": {
                        "email": "chris@openagents.com"
                    }
                })),
                expires: unix_time_millis().expect("now") + 60_000,
                account_id: Some(String::from("acct-email")),
            })
            .expect("save auth");

        let status = store.status().expect("status");
        assert_eq!(
            status.selected_account_email.as_deref(),
            Some("chris@openagents.com")
        );
        assert_eq!(
            status.accounts[0].user_email.as_deref(),
            Some("chris@openagents.com")
        );
    }

    #[test]
    fn routing_plan_prefers_account_with_more_headroom() {
        let temp = tempdir().expect("temp dir");
        let backend = FakeAppleFmServer::from_handler(|request: FakeHttpRequest| {
            assert_eq!(request.path, "/wham/usage");
            let used_percent = if request.raw.contains("ChatGPT-Account-Id: acct-low") {
                12
            } else if request.raw.contains("ChatGPT-Account-Id: acct-high") {
                76
            } else {
                panic!("missing account header in request: {}", request.raw);
            };
            FakeHttpResponse::json_ok(serde_json::json!({
                "plan_type": "pro",
                "rate_limit": {
                    "allowed": true,
                    "limit_reached": false,
                    "primary_window": {
                        "used_percent": used_percent,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 3600,
                        "reset_at": 1776793745
                    }
                }
            }))
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: String::from("https://auth.openai.test"),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(2),
                polling_safety_margin: Duration::from_millis(1),
                backend_api_base_url: backend.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(0),
            },
        )
        .expect("controller");
        controller
            .store()
            .save_account(
                &OpenAiCodexAuthRecord {
                    refresh: String::from("refresh-high"),
                    access: String::from("access-high"),
                    expires: unix_time_millis().expect("now") + 60_000,
                    account_id: Some(String::from("acct-high")),
                },
                Some(String::from("high")),
            )
            .expect("save high account");
        controller
            .store()
            .save_account(
                &OpenAiCodexAuthRecord {
                    refresh: String::from("refresh-low"),
                    access: String::from("access-low"),
                    expires: unix_time_millis().expect("now") + 60_000,
                    account_id: Some(String::from("acct-low")),
                },
                Some(String::from("low")),
            )
            .expect("save low account");

        let plan = controller.routing_plan(None).expect("routing plan");
        match plan.routes.first() {
            Some(OpenAiCodexRoute::SubscriptionAccount(route)) => {
                assert_eq!(route.account_key, "acct:acct-low");
                assert_eq!(route.label.as_deref(), Some("low"));
            }
            other => panic!("unexpected first route: {other:?}"),
        }
    }

    #[test]
    fn routing_plan_uses_optional_api_key_fallback_when_accounts_are_limited() {
        let temp = tempdir().expect("temp dir");
        let backend = FakeAppleFmServer::from_handler(|request: FakeHttpRequest| {
            assert_eq!(request.path, "/wham/usage");
            FakeHttpResponse::json_ok(serde_json::json!({
                "plan_type": "pro",
                "rate_limit": {
                    "allowed": false,
                    "limit_reached": true,
                    "primary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 1800,
                        "reset_at": 1776793745
                    }
                }
            }))
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: String::from("https://auth.openai.test"),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(2),
                polling_safety_margin: Duration::from_millis(1),
                backend_api_base_url: backend.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(0),
            },
        )
        .expect("controller");
        controller
            .store()
            .save(&OpenAiCodexAuthRecord {
                refresh: String::from("refresh-limited"),
                access: String::from("access-limited"),
                expires: unix_time_millis().expect("now") + 60_000,
                account_id: Some(String::from("acct-limited")),
            })
            .expect("save limited account");

        // SAFETY: the test uses a unique env var and restores process state before exit.
        unsafe {
            std::env::set_var("PROBE_TEST_OPENAI_API_KEY_FALLBACK", "probe-test-key");
        }
        let plan = controller
            .routing_plan(Some("PROBE_TEST_OPENAI_API_KEY_FALLBACK"))
            .expect("routing plan");
        assert!(plan.api_key_fallback_available);
        match plan.routes.first() {
            Some(OpenAiCodexRoute::ApiKeyFallback(route)) => {
                assert_eq!(route.env_var, "PROBE_TEST_OPENAI_API_KEY_FALLBACK");
                assert_eq!(route.api_key, "probe-test-key");
            }
            other => panic!("unexpected first route: {other:?}"),
        }
        // SAFETY: this test removes the unique env key it created.
        unsafe {
            std::env::remove_var("PROBE_TEST_OPENAI_API_KEY_FALLBACK");
        }
    }

    #[test]
    fn routing_plan_uses_optional_api_key_fallback_when_auth_state_is_missing() {
        let temp = tempdir().expect("temp dir");
        let controller = OpenAiCodexAuthController::new(temp.path()).expect("controller");
        // SAFETY: this test uses a unique env var and restores process state before exit.
        unsafe {
            std::env::set_var("PROBE_TEST_OPENAI_API_KEY_ONLY", "probe-test-key");
        }
        let plan = controller
            .routing_plan(Some("PROBE_TEST_OPENAI_API_KEY_ONLY"))
            .expect("routing plan");
        assert!(plan.api_key_fallback_available);
        match plan.routes.first() {
            Some(OpenAiCodexRoute::ApiKeyFallback(route)) => {
                assert_eq!(route.env_var, "PROBE_TEST_OPENAI_API_KEY_ONLY");
                assert_eq!(route.api_key, "probe-test-key");
            }
            other => panic!("unexpected first route: {other:?}"),
        }
        // SAFETY: this test removes the unique env key it created.
        unsafe {
            std::env::remove_var("PROBE_TEST_OPENAI_API_KEY_ONLY");
        }
    }

    #[test]
    fn routing_plan_returns_typed_error_when_all_accounts_are_limited_without_fallback() {
        let temp = tempdir().expect("temp dir");
        let backend = FakeAppleFmServer::from_handler(|request: FakeHttpRequest| {
            assert_eq!(request.path, "/wham/usage");
            FakeHttpResponse::json_ok(serde_json::json!({
                "plan_type": "pro",
                "rate_limit": {
                    "allowed": false,
                    "limit_reached": true,
                    "primary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 1800,
                        "reset_at": 1776793745
                    }
                }
            }))
        });
        let controller = OpenAiCodexAuthController::with_config(
            temp.path(),
            OpenAiAuthConfig {
                issuer: String::from("https://auth.openai.test"),
                client_id: String::from(DEFAULT_OPENAI_CLIENT_ID),
                oauth_port: 0,
                browser_timeout: Duration::from_secs(2),
                polling_safety_margin: Duration::from_millis(1),
                backend_api_base_url: backend.base_url().to_string(),
                rate_limit_cache_ttl: Duration::from_secs(0),
            },
        )
        .expect("controller");
        controller
            .store()
            .save(&OpenAiCodexAuthRecord {
                refresh: String::from("refresh-limited"),
                access: String::from("access-limited"),
                expires: unix_time_millis().expect("now") + 60_000,
                account_id: Some(String::from("acct-limited")),
            })
            .expect("save limited account");

        let error = controller
            .routing_plan(Some("PROBE_TEST_OPENAI_API_KEY_FALLBACK_MISSING"))
            .expect_err("limited accounts without fallback should error");
        assert!(matches!(
            error,
            OpenAiAuthError::AllCodexAccountsRateLimited { .. }
        ));
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
