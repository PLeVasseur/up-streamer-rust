/********************************************************************************
 * Copyright (c) 2026 Contributors to the Eclipse Foundation
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

//! Data-plane layer.
//!
//! Owns ingress listener registration/unregistration and egress worker pooling.
//! This layer translates resolved route policy into concrete ingress/egress
//! dispatch execution paths.
//!
//! ```
//! use std::sync::Arc;
//! use up_rust::core::usubscription::USubscription;
//! use up_rust::UTransport;
//! use up_streamer::{Endpoint, UStreamer};
//! use usubscription_static_file::USubscriptionStaticFile;
//!
//! # use up_rust::MockTransport;
//! #
//! # fn mock_transport() -> Arc<dyn UTransport> {
//! #     let mut transport = MockTransport::default();
//! #     transport.expect_do_send().returning(|_message| Ok(()));
//! #     transport
//! #         .expect_do_register_listener()
//! #         .returning(|_source_filter, _sink_filter, _listener| Ok(()));
//! #     transport
//! #         .expect_do_unregister_listener()
//! #         .returning(|_source_filter, _sink_filter, _listener| Ok(()));
//! #     Arc::new(transport)
//! # }
//! #
//! # tokio::runtime::Runtime::new().unwrap().block_on(async {
//! let usubscription: Arc<dyn USubscription> = Arc::new(USubscriptionStaticFile::new(
//!     "../utils/usubscription-static-file/static-configs/testdata.json".to_string(),
//! ));
//! let mut streamer = UStreamer::new("data-plane-doc", 16, usubscription).await.unwrap();
//! let ingress_transport: Arc<dyn UTransport> = mock_transport();
//! let egress_transport: Arc<dyn UTransport> = mock_transport();
//! let ingress = Endpoint::new("ingress", "authority-a", ingress_transport);
//! let egress = Endpoint::new("egress", "authority-b", egress_transport);
//!
//! // Registering and deleting routes creates/drops ingress listeners and egress workers.
//! streamer.add_route(ingress.clone(), egress.clone()).await.unwrap();
//! streamer.delete_route(ingress, egress).await.unwrap();
//! # });
//! ```

pub(crate) mod egress_pool;
pub(crate) mod egress_worker;
pub(crate) mod ingress_listener;
pub(crate) mod ingress_registry;
