pub mod compose;
pub mod env;
pub mod init;
pub mod updater;


pub static PACKAGE: &[u8] = include_bytes!(env!("COMPRESSED_FILE"));
