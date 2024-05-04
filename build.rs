use std::{fs::File, io::BufReader};

use flate2::{write::GzEncoder, Compression};

fn main() {
    // Just an example..
    let mut input = BufReader::new(File::open("./docker/docker-compose-base.yml").unwrap());
    let output = File::create("./compressed.zip").unwrap();
    let mut encoder = GzEncoder::new(output, Compression::default());
    std::io::copy(&mut input, &mut encoder).unwrap();
    let _output = encoder.finish().unwrap();

    println!("cargo:rustc-env=COMPRESSED_FILE={}", "../compressed.zip");
    println!("cargo:rerun-if-changed=docker")
}
