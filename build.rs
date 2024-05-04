use flate2::{write::GzEncoder, Compression};
use std::fs::File;

fn main() {
    let output = File::create("./compressed.tar.gz").unwrap();
    let encoder = GzEncoder::new(output, Compression::default());
    let mut tar = tar::Builder::new(encoder);
    tar.append_dir_all("./", "./package").unwrap();
    tar.finish().unwrap();

    println!("cargo:rustc-env=COMPRESSED_FILE={}", "../compressed.tar.gz");
    println!("cargo:rerun-if-changed=docker,package")
}
