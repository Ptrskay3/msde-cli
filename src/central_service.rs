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

use anyhow::Context;
use reqwest::header::{HeaderMap, HeaderName};

pub static X_MSDE_CLI_VERSION: HeaderName = HeaderName::from_static("x-msde-cli-version");

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

    // This is probably how we'd like to setup callable endpoints..
    pub async fn endpoint(&self, parameter: &str) -> anyhow::Result<()> {
        let url = format!("{}/url/{parameter}", self.api_url);

        #[derive(serde::Deserialize)]
        struct Response {}

        self.client
            .get(url)
            .send()
            .await
            .context("call endpoint")?
            .json::<Response>()
            .await
            .context("parse response")?;

        Ok(())
    }
}
