//! Plan:
//! - Central service:
//!   - Generates tokens (hopefully JWT) that contain basic info: user, accessible versions, and allows us to download those images.
//!   - Revoke access to issued tokens.
//! - This tool:
//!   - Decodes this JWT, downloads the images through the service.
//!   - login, logout via the tokens (maybe allow multiple login profiles, something like AWS)
//!   - The client SDK probably needs to be in this central service too (we can start as an embedded binary)
//!
//! This is currently just a sketch.
#![allow(dead_code)]

use std::{collections::HashMap, time::Duration};

#[cfg(all(feature = "local_auth", debug_assertions))]
use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderName};

pub static X_MSDE_CLI_VERSION: HeaderName = HeaderName::from_static("x-msde-cli-version");
static X_ACCESS_TOKEN: HeaderName = HeaderName::from_static("x-access-token");

#[derive(Clone)]
pub struct MerigoApiClient {
    client: reqwest::Client,
    api_url: String,
    access_token: Option<AccessToken>,
}

#[derive(Clone)]
pub struct AccessToken {
    token: String,
}

impl MerigoApiClient {
    pub fn new(api_url: String, access_token: Option<AccessToken>, self_version: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .default_headers(
                    HeaderMap::try_from(&HashMap::from([(
                        X_MSDE_CLI_VERSION.clone(),
                        self_version,
                    )]))
                    .unwrap(),
                )
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap(),
            api_url,
            access_token,
        }
    }

    #[cfg(all(feature = "local_auth", debug_assertions))]
    pub async fn register(&self, name: &str) -> anyhow::Result<String> {
        let url = format!("{}/register", self.api_url);

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            token: String,
        }

        let token = self
            .client
            .post(url)
            .json(&serde_json::json!({"name": name}))
            .send()
            .await
            .context("call endpoint")?
            .json::<TokenResponse>()
            .await
            .context("parse response")?
            .token;

        Ok(token)
    }

    #[cfg(all(feature = "local_auth", debug_assertions))]
    pub async fn login(&self, token: &str) -> anyhow::Result<String> {
        let url = format!("{}/auth", self.api_url);

        #[derive(serde::Deserialize)]
        struct LoginResponse {
            name: String,
        }

        #[derive(serde::Deserialize, Debug)]
        struct ErrorResponse {
            error: String,
        }

        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Response {
            Ok(LoginResponse),
            Error(ErrorResponse),
        }

        match self
            .client
            .get(url)
            .header(X_ACCESS_TOKEN.clone(), token)
            .send()
            .await
            .context("call endpoint")?
            .json::<Response>()
            .await
            .context("parse body")?
        {
            Response::Ok(l) => Ok(l.name),
            Response::Error(e) => {
                tracing::error!(?e, "unauthorized");
                anyhow::bail!("unauthorized")
            }
        }
    }
}
