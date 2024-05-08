//! Plan:
//! - Central service:
//!   - Generates tokens (hopefully JWT) that contain basic info: user, accessible versions, and allows us to download those images.
//!   - Revoke access to issued tokens.
//! - This tool:
//!   - Decodes this JWT, downloads the images through the service.
//!   - login, logout via the tokens (maybe allow multiple login profiles, something like AWS)
//!   - The client SDK probably needs to be in this central service too (we can start as an embedded binary)
