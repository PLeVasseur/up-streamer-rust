//! Subscription bootstrap runtime integration helpers.

use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::task;
use up_rust::core::usubscription::{
    FetchSubscriptionsRequest, FetchSubscriptionsResponse, USubscription,
};

const THREAD_NUM: usize = 10;

lazy_static! {
    static ref CB_RUNTIME: Runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(THREAD_NUM)
        .enable_all()
        .build()
        .expect("Unable to create callback runtime");
}

pub(crate) fn fetch_subscriptions(
    usubscription: Arc<dyn USubscription>,
    fetch_request: FetchSubscriptionsRequest,
) -> FetchSubscriptionsResponse {
    task::block_in_place(|| {
        CB_RUNTIME
            .block_on(usubscription.fetch_subscriptions(fetch_request))
            .expect("Failed to fetch subscriptions")
    })
}
