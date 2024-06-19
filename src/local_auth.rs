use anyhow::Context;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{request::Parts, HeaderName, HeaderValue},
    routing::{get, post},
    Json, Router,
};
use jsonwebtoken::{DecodingKey, TokenData, Validation};
use reqwest::StatusCode;
use serde::Deserialize;
use std::{
    collections::HashSet,
    io::Read,
    sync::{Arc, Mutex, OnceLock},
};
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

const HMAC_KEY: &str =
    "KvQPHOtiRc3RECvpokvBfOVSb8pyynHdPVXVvyjVonXX8lrS8jT8Z/pzOlHBLlA9AIO0T9rR60bg2zKtXItkDA==";
static SESSION_LENGTH: OnceLock<time::Duration> = OnceLock::new();

#[allow(clippy::declare_interior_mutable_const)]
const X_ACCESS_TOKEN: HeaderName = HeaderName::from_static("x-access-token");

#[derive(Clone)]
pub struct AppState {
    pub authorized_tokens: Arc<Mutex<HashSet<String>>>,
}

pub async fn run_local_auth_server() -> anyhow::Result<()> {
    let mut s = String::new();
    let mut f = std::fs::File::open("local_authorized_tokens.json")?;
    f.read_to_string(&mut s)?;
    #[derive(Deserialize)]
    struct BuiltInKeys {
        tokens: HashSet<String>,
    }

    let built_in_keys: BuiltInKeys = serde_json::from_str(&s)?;
    let app_state = AppState {
        authorized_tokens: Arc::new(Mutex::new(built_in_keys.tokens)),
    };
    let router = Router::<AppState>::new()
        .route("/register", post(register_client))
        .route("/auth", get(auth_client))
        .layer(tower::ServiceBuilder::new().layer(TraceLayer::new_for_http()))
        .with_state(app_state);

    let addr = "0.0.0.0:8765";
    let listener = TcpListener::bind(addr).await.unwrap();
    tracing::info!(%addr, "local auth server started");
    axum::serve(listener, router)
        .await
        .context("failed to start the server")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AuthUserClaims {
    name: String,
    exp: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthUser {
    pub name: String,
}

impl AuthUser {
    pub fn to_jwt(&self) -> String {
        let session_length = SESSION_LENGTH.get_or_init(|| time::Duration::days(30));
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &AuthUserClaims {
                name: self.name.clone(),
                exp: (OffsetDateTime::now_utc() + *session_length).unix_timestamp(),
            },
            &jsonwebtoken::EncodingKey::from_secret(HMAC_KEY.as_bytes()),
        )
        .expect("HMAC signing should be infallible")
    }

    pub fn from_authorization(
        auth_header: &HeaderValue,
        authorized_tokens: &Arc<Mutex<HashSet<String>>>,
    ) -> anyhow::Result<String> {
        let token = auth_header.to_str().map_err(|_| {
            tracing::debug!("Authorization header is not UTF-8");
            anyhow::Error::msg("unauthorized")
        })?;
        if authorized_tokens.lock().unwrap().contains(token) {
            return Ok(String::from("local-built-in-user"));
        }

        let decoding = DecodingKey::from_secret(HMAC_KEY.as_bytes());
        let validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        let TokenData { claims, .. } =
            jsonwebtoken::decode::<AuthUserClaims>(token, &decoding, &validation)
                .map_err(|_| anyhow::Error::msg("unauthorized"))?;

        if claims.exp < OffsetDateTime::now_utc().unix_timestamp() {
            tracing::debug!("token expired");
            return Err(anyhow::Error::msg("token expired"));
        }

        Ok(claims.name)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Token {
    token: String,
}

async fn register_client(Json(auth_user): Json<AuthUser>) -> Json<Token> {
    let token = auth_user.to_jwt();
    Json(Token { token })
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct ErrorResponse {
    error: String,
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = (StatusCode, Json<ErrorResponse>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(access_token) = parts.headers.get(X_ACCESS_TOKEN) {
            let state = AppState::from_ref(state);
            let name = AuthUser::from_authorization(access_token, &state.authorized_tokens)
                .map_err(|_| {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "invalid token".to_string(),
                        }),
                    )
                })?;
            Ok(AuthUser { name })
        } else {
            Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "unauthorized".to_string(),
                }),
            ))
        }
    }
}

async fn auth_client(auth_user: AuthUser) -> Json<AuthUser> {
    Json(auth_user)
}
