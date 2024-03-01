use async_std::task;
use async_trait::async_trait;
use log::trace;
use up_rust::transport::datamodel::UTransport;
use up_rust::uprotocol::{UAuthority, UMessage, UStatus, UUri};

fn transport_listener(result: Result<UMessage, UStatus>) {

}

fn streamer_uuri_from_uauthority(authority: &UAuthority) -> UUri {
    UUri {
        authority: Some(authority.clone()).into(),
        ..Default::default()
    }
}

#[async_trait]
pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;

    fn start(
        &self,
        authorities: Vec<UAuthority>,
    ) {
        trace!("entered create_and_setup");

        let utransport = self.build();

        task::spawn_local(async move {
            for authority in &authorities {
                let register_success = utransport.register_listener(streamer_uuri_from_uauthority(authority), Box::new(transport_listener)).await;
            }
            // TODO: Add receiver queue in here
            let send_success = utransport.send(Default::default()).await;

            async_std::future::pending::<()>().await;
        });
    }
}
