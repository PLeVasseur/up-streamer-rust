// SPDX-License-Identifier: Apache-2.0
//! DDS subscriber role binary.

#[path = "common/dds.rs"]
mod dds;

#[tokio::main]
async fn main() -> Result<(), up_rust::UStatus> {
    dds::run(dds::Role::Subscriber).await
}
