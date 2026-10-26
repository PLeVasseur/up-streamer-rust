// SPDX-License-Identifier: Apache-2.0
//! DDS notifyee role binary.

#[path = "common/dds.rs"]
mod dds;

#[tokio::main]
async fn main() -> Result<(), up_rust::UStatus> {
    dds::run(dds::Role::Notifyee).await
}
