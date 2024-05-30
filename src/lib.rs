pub mod central_service;
pub mod compose;
pub mod env;
pub mod game;
pub mod init;
pub mod parsing;
pub mod updater;
pub mod utils;

pub static PACKAGE: &[u8] = include_bytes!(env!("COMPRESSED_FILE"));
