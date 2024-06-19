use flate2::{write::GzEncoder, Compression};
use std::fs::File;

fn main() {
    println!("cargo:rerun-if-changed=package");
    println!("cargo:rerun-if-changed=template");
    let package = File::create("./compressed_package.tar.gz").unwrap();
    let template = File::create("./compressed_template.tar.gz").unwrap();
    let package_encoder = GzEncoder::new(package, Compression::default());
    let template_encoder = GzEncoder::new(template, Compression::default());
    let mut template_tar = tar::Builder::new(template_encoder);
    let mut package_tar = tar::Builder::new(package_encoder);
    package_tar.append_dir_all("./", "./package").unwrap();
    package_tar.finish().unwrap();
    template_tar.append_dir_all("./", "./template").unwrap();
    template_tar.finish().unwrap();

    println!("cargo:rustc-env=PACKAGE_COMPRESSED_FILE=../compressed_package.tar.gz");
    println!("cargo:rustc-env=TEMPLATE_COMPRESSED_FILE=../compressed_template.tar.gz");
}
