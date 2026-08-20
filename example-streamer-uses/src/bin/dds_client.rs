// SPDX-License-Identifier: Apache-2.0
//! DDS client role binary.

#[path = "common/dds.rs"]
mod dds;

#[tokio::main]
async fn main() -> Result<(), up_rust::UStatus> {
    dds::run(dds::Role::Client).await
}
