pub mod compose;
pub mod env;
pub mod init;
pub mod updater;

pub static FILE: &[u8] = include_bytes!(env!("COMPRESSED_FILE"));
