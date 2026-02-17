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

//! Control-plane layer.
//!
//! Owns route-registration lifecycle semantics and the route-table identity model.
//! This layer is responsible for idempotent insert/remove behavior and rollback-safe
//! transitions when listener registration fails.
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
//! let mut streamer = UStreamer::new("control-plane-doc", 16, usubscription).await.unwrap();
//! let left_transport: Arc<dyn UTransport> = mock_transport();
//! let right_transport: Arc<dyn UTransport> = mock_transport();
//! let left = Endpoint::new("left", "left-authority", left_transport);
//! let right = Endpoint::new("right", "right-authority", right_transport);
//!
//! // The control plane ensures duplicate insert/remove transitions stay idempotent.
//! streamer.add_route(left.clone(), right.clone()).await.unwrap();
//! assert!(streamer.add_route(left.clone(), right.clone()).await.is_err());
//! streamer.delete_route(left.clone(), right.clone()).await.unwrap();
//! assert!(streamer.delete_route(left, right).await.is_err());
//! # });
//! ```

pub(crate) mod route_lifecycle;
pub(crate) mod route_table;
pub(crate) mod transport_identity;
