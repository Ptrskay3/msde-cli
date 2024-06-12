use anyhow::Context;
use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts, State},
    http::{request::Parts, HeaderName, HeaderValue},
    routing::{get, post},
    Json, Router,
};
use jsonwebtoken::{DecodingKey, TokenData, Validation};
use reqwest::StatusCode;
use std::{
    collections::HashMap,
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
    pub users: Arc<Mutex<HashMap<String, String>>>,
}

pub async fn run_local_auth_server() -> anyhow::Result<()> {
    let app_state = AppState {
        users: Arc::new(Mutex::new(HashMap::new())),
    };
    let router = Router::<AppState>::new()
        .route("/register", post(register_client))
        .route("/auth", get(auth_client))
        .layer(tower::ServiceBuilder::new().layer(TraceLayer::new_for_http()))
        .with_state(app_state);

    let listener = TcpListener::bind("0.0.0.0:8765").await.unwrap();
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

    pub fn from_authorization(auth_header: &HeaderValue) -> anyhow::Result<String> {
        let token = auth_header.to_str().map_err(|_| {
            tracing::debug!("Authorization header is not UTF-8");
            anyhow::Error::msg("unauthorized")
        })?;

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

async fn register_client(
    State(state): State<AppState>,
    Json(auth_user): Json<AuthUser>,
) -> Json<Token> {
    let token = auth_user.to_jwt();
    let mut users = state.users.lock().unwrap();
    users.insert(token.clone(), auth_user.name);
    Json(Token { token })
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if let Some(access_token) = parts.headers.get(X_ACCESS_TOKEN) {
            let _app_state = AppState::from_ref(state);
            let name = AuthUser::from_authorization(access_token)
                .map_err(|_| (StatusCode::BAD_REQUEST, "invalid token".to_string()))?;
            Ok(AuthUser { name })
        } else {
            Err((StatusCode::UNAUTHORIZED, "unauthorized".into()))
        }
    }
}

async fn auth_client(auth_user: AuthUser) -> Json<AuthUser> {
    Json(auth_user)
}
