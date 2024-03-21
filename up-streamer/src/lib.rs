//! # up-streamer
//!
//! `up-streamer` implements the `UStreamer` spec to allow bridging between different
//! transports.

mod route;
mod sender_wrapper;
mod ustreamer;
mod utransport_builder;
mod utransport_router;

pub use route::Route;

pub use utransport_builder::UTransportBuilder;

pub use ustreamer::UStreamer;

pub use utransport_router::{UTransportRouter, UTransportRouterHandle};
