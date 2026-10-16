use std::env;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let proto = root.join("../../proto/ph.proto");

    // Use the vendored protoc so no system protoc is needed.
    env::set_var(
        "PROTOC",
        protoc_bin_vendored::protoc_bin_path().expect("vendored protoc unavailable"),
    );

    let mut config = prost_build::Config::new();
    // nanopb.proto imports google/protobuf/descriptor.proto; have prost build
    // the well-known types alongside so the schema stays nanopb-annotated.
    config.compile_well_known_types();
    config
        .compile_protos(&[proto], &[root.join("../../proto")])
        .expect("prost proto compile failed");

    println!("cargo:rerun-if-changed=../../proto/ph.proto");
    println!("cargo:rerun-if-changed=../../proto/nanopb.proto");
}