pub mod central_service;
pub mod cli;
pub mod compose;
pub mod env;
pub mod game;
pub mod hooks;
pub mod init;
#[cfg(all(feature = "local_auth", debug_assertions))]
pub mod local_auth;
pub mod parsing;
pub mod updater;
pub mod utils;

pub const LATEST: &str = "latest";
pub const USER: &str = "merigo-client";
pub const DEFAULT_DURATION: i64 = 12;
pub const MERIGO_UPSTREAM_VERSION: &str = env!("MERIGO_UPSTREAM_VERSION");

pub const REPOS_AND_IMAGES: &[&str; 5] = &[
    "merigo_dev_packages/compiler-vm-dev",
    "merigo_dev_packages/msde-vm-dev",
    "merigo_dev_packages/bot-vm-dev",
    "web3_services/web3_services_dev",
    "web3_services/web3_consumer_dev",
];

pub static PACKAGE: &[u8] = include_bytes!(env!("PACKAGE_COMPRESSED_FILE"));
pub static TEMPLATE: &[u8] = include_bytes!(env!("TEMPLATE_COMPRESSED_FILE"));
