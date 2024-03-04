use async_std::task;
use async_trait::async_trait;
use log::trace;
use up_rust::transport::datamodel::UTransport;
use up_rust::uprotocol::{UAuthority, UMessage, UStatus, UUri};

fn transport_listener(result: Result<UMessage, UStatus>) {
    // TODO: Implement

    // TODO:
    //   1. Check if we have seen this message already based on UMessage.attributes.id
    //    => If we have, we have already transmitted this message once, we can drop it
    //    => Must pass in reference to the LRU cache
    //   2. If this message is intended for our device, then place it into the ingress queue
    //      otherwise place it into the egress queue
    //    => Must pass in host_transport boolean
    //    => Must pass in ingress_sender
    //    => Must pass in egress_sender
}

async fn transmit_loop() {
    // TODO: Implement

    // TODO:
    //   1. Loop over and consume from transmit_queue
    //     => Must pass in reference to utransport_receiver
    //   2. Send out over transport
    //     => Must pass in ownership of Box<dyn UTransport>
}

fn streamer_uuri_from_uauthority(authority: &UAuthority) -> UUri {
    UUri {
        authority: Some(authority.clone()).into(),
        ..Default::default()
    }
}

pub trait UTransportBuilder: Send + Sync {
    fn build(&self) -> Box<dyn UTransport>;

    fn start(&self, authorities: Vec<UAuthority>) {
        trace!("entered create_and_setup");

        let utransport = self.build();

        task::spawn_local(async move {
            for authority in &authorities {
                let register_success = utransport
                    .register_listener(
                        streamer_uuri_from_uauthority(authority),
                        Box::new(transport_listener),
                    )
                    .await;
            }
            // TODO: Add receiver queue in here
            let send_success = utransport.send(Default::default()).await;

            async_std::future::pending::<()>().await;
        });
    }
}
