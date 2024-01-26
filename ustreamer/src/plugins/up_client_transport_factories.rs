/********************************************************************************
 * Copyright (c) 2023 Contributors to the Eclipse Foundation
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

use crate::plugins::types::TransportType;
use crate::plugins::up_client_transport::UpClientTransport;
use crate::plugins::up_client_transport::UpClientTransportFactory;
use async_std::pin::Pin;
use async_std::sync::Mutex;
use std::future::Future;
use std::sync::Arc;
use uprotocol_rust_transport_sommr::UTransportSommr;
use zenoh::config::WhatAmI;

pub struct UTransportSommrFactory {}
impl UpClientTransportFactory for UTransportSommrFactory {
    fn transport_type(&self) -> &'static TransportType {
        &TransportType::UpClientSommr
    }
    fn create_up_client(
        &self,
    ) -> Box<
        dyn FnOnce()
            -> Pin<Box<dyn Future<Output = Arc<Mutex<Box<dyn UpClientTransport>>>> + Send>>,
    > {
        Box::new(|| {
            Box::pin(async move {
                let mut up_client_config = zenoh::config::Config::default();
                up_client_config
                    .set_mode(Some(WhatAmI::Peer))
                    .expect("Unable to configure as Peer");
                let up_client: Arc<Mutex<Box<dyn UpClientTransport>>> =
                    Arc::new(Mutex::new(Box::new(
                        UTransportSommr::new_from_config(up_client_config)
                            .await
                            .unwrap(),
                    )));
                up_client
            })
        })
    }
}
