use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::thread;
use async_std::sync::Mutex;
use async_std::task;
use async_trait::async_trait;
use log::trace;
use uprotocol_sdk::transport::datamodel::UTransport;
use uprotocol_sdk::uprotocol::{UAuthority, UMessage, UStatus, UUri};

fn transport_listener(result: Result<UMessage, UStatus>) {

}

fn streamer_uuri_from_uauthority(authority: &UAuthority) -> UUri {
    UUri {
        authority: Some(authority.clone()),
        ..Default::default()
    }
}

#[async_trait]
pub trait UTransportBuilder: Send + Sync {
    fn create_up_client(&self) -> Box<dyn UTransport>;

    fn create_and_setup(
        &self,
        authorities: Vec<UAuthority>,
    ) {
        trace!("entered create_and_setup");

        let utransport = self.create_up_client();

        task::spawn_local(async move {
            for authority in &authorities {
                let register_success = utransport.register_listener(streamer_uuri_from_uauthority(authority), Box::new(transport_listener)).await;
            }
            // TODO: Add receiver queue in here
            let send_success = utransport.send(Default::default(), Default::default(), Default::default()).await;

            async_std::future::pending::<()>().await;
        });
    }
}
