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

fn uuri_from_uauthority(authority: &UAuthority) -> UUri {
    UUri {
        authority: Some(authority.clone()),
        ..Default::default()
    }
}

pub type UTransportBuilderFunction<'a> =
Box<dyn FnOnce() -> Box<dyn UTransport + 'a> + 'a>;

#[async_trait]
pub trait UTransportBuilder: Send + Sync {
    fn create_up_client(&self) -> UTransportBuilderFunction;

    fn create_and_setup(
        &self,
        authorities: Vec<UAuthority>,
    ) {
        trace!("entered create_and_setup");

        let utransport =
            self.create_up_client()();
        for authority in &authorities {
            let register_success = task::block_on(utransport.register_listener(uuri_from_uauthority(authority), Box::new(transport_listener)));
        }

        loop {
            let send_success = task::block_on(utransport.send(Default::default(), Default::default(), Default::default()));
        }
    }
}
