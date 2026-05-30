use crate::dlm_error::DlmError;
use crate::user_agents::{UserAgent, random_user_agent};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy};
use std::time::Duration;

/// Sent on every request when the user does not pass `--user-agent` or
/// `--random-user-agent`. Identifies the tool to server admins (better than
/// reqwest's generic default) without impersonating a browser.
const DEFAULT_USER_AGENT: &str = concat!("dlm/", env!("CARGO_PKG_VERSION"));

pub struct ClientConfig<'a> {
    pub user_agent: Option<&'a UserAgent>,
    pub proxy: Option<&'a str>,
    pub connection_timeout_secs: u32,
    pub insecure: bool,
    pub basic_auth: Option<(&'a str, &'a str)>,
    pub headers: &'a [(String, String)],
}

pub fn make_client(config: &ClientConfig<'_>, redirect: bool) -> Result<Client, DlmError> {
    let connect_timeout = Duration::from_secs(u64::from(config.connection_timeout_secs));
    // `connect_timeout` only bounds establishing the connection.
    // `read_timeout` is a coarse backstop for a server that accepts the connection then goes
    // silent — chiefly before sending the response headers, which the download
    // loop's per-chunk timeout never reaches. It is set to 2x so it stays out
    // of the way of that tighter per-chunk timeout during normal body streaming.
    let read_timeout = connect_timeout * 2;
    let client_builder = Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .danger_accept_invalid_certs(config.insecure);

    let client_builder = match config.user_agent {
        Some(UserAgent::CustomUserAgent(ua)) => client_builder.user_agent(ua),
        Some(UserAgent::RandomUserAgent) => client_builder.user_agent(random_user_agent()),
        None => client_builder.user_agent(DEFAULT_USER_AGENT),
    };

    let client_builder = match config.proxy {
        Some(p) => client_builder.proxy(Proxy::all(p)?),
        None => client_builder,
    };

    // basic-auth goes in first so a custom `-H 'Authorization: …'` can override it.
    let default_headers = build_default_headers(config)?;
    let client_builder = if default_headers.is_empty() {
        client_builder
    } else {
        client_builder.default_headers(default_headers)
    };

    let client_builder = if redirect {
        // reqwest defaults to 10 redirects
        client_builder.redirect(Policy::default())
    } else {
        client_builder.redirect(Policy::none())
    };

    Ok(client_builder.build()?)
}

fn build_default_headers(config: &ClientConfig<'_>) -> Result<HeaderMap, DlmError> {
    let mut headers = HeaderMap::new();

    if let Some((user, pass)) = config.basic_auth {
        let encoded = BASE64.encode(format!("{user}:{pass}"));
        let value = HeaderValue::from_str(&format!("Basic {encoded}")).map_err(|e| {
            DlmError::CliArgumentError {
                message: format!("invalid basic-auth value: {e}"),
            }
        })?;
        headers.insert(AUTHORIZATION, value);
    }

    for (name, value) in config.headers {
        let header_name = name
            .parse::<HeaderName>()
            .map_err(|e| DlmError::CliArgumentError {
                message: format!("invalid header name '{name}': {e}"),
            })?;
        let header_value =
            HeaderValue::from_str(value).map_err(|e| DlmError::CliArgumentError {
                message: format!("invalid header value for '{name}': {e}"),
            })?;
        headers.insert(header_name, header_value);
    }

    Ok(headers)
}
