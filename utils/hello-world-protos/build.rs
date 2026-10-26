/********************************************************************************
 * Copyright (c) 2024 Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 ********************************************************************************/

//! Generate the Hello World example messages from the crate's frozen schema inputs.
use protobuf_codegen::Customize;
use std::env;
use std::path::PathBuf;

fn main() {
    let proto_folder =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest path"))
            .join("proto");
    let inputs = [
        "google/type/timeofday.proto",
        "uprotocol/uoptions.proto",
        "example/hello_world/v1/hello_world_topics.proto",
        "example/hello_world/v1/hello_world_service.proto",
    ];
    println!("cargo:rerun-if-changed=proto");

    protobuf_codegen::Codegen::new()
        .protoc()
        .protoc_path(&protoc_bin_vendored::protoc_bin_path().expect("vendored protoc"))
        .customize(Customize::default().tokio_bytes(true))
        .include(&proto_folder)
        .inputs(inputs.map(|relative| proto_folder.join(relative)))
        .cargo_out_dir("helloworld")
        .run_from_script();
}
