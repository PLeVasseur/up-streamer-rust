use crate::route::Route;
use up_rust::UStatus;

pub struct UStreamer;

impl UStreamer {
    pub async fn add_forwarding_rule(&self, r#in: Route, out: Route) -> Result<(), UStatus> {
        println!("UStreamer::add_forwarding_rule()");
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        out.get_transport_router_handle()
            .register(r#in.get_authority(), in_message_sender.clone())
            .await
    }

    pub async fn delete_forwarding_rule(&self, r#in: Route, out: Route) -> Result<(), UStatus> {
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        out.get_transport_router_handle()
            .unregister(r#in.get_authority(), in_message_sender.clone())
            .await
    }
}

mod tests {
    use crate::route::Route;
    use crate::ustreamer::UStreamer;
    use crate::utransport_builder::UTransportBuilder;
    use crate::utransport_router::UTransportRouter;
    use async_std::task;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use up_rust::{Number, UAuthority, UMessage, UStatus, UTransport, UUri};

    pub struct UPClientFoo;

    #[async_trait]
    impl UTransport for UPClientFoo {
        async fn send(&self, message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
        ) -> Result<String, UStatus> {
            println!("UPClientFoo: topic: {:?}", topic);
            Ok("abc".to_string())
        }

        async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
            todo!()
        }
    }

    impl UPClientFoo {
        pub fn new() -> Self {
            Self {}
        }
    }
    pub struct UTransportBuilderFoo;
    impl UTransportBuilder for UTransportBuilderFoo {
        fn build(&self) -> Box<dyn UTransport> {
            let utransport_foo: Box<dyn UTransport> = Box::new(UPClientFoo::new());
            utransport_foo
        }
    }

    impl UTransportBuilderFoo {
        pub fn new() -> Self {
            Self {}
        }
    }

    pub struct UPClientBar;

    #[async_trait]
    impl UTransport for UPClientBar {
        async fn send(&self, message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
        ) -> Result<String, UStatus> {
            println!("UPClientBar: topic: {:?}", topic);
            Ok("abc".to_string())
        }

        async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
            todo!()
        }
    }

    impl UPClientBar {
        pub fn new() -> Self {
            Self {}
        }
    }
    pub struct UTransportBuilderBar;
    impl UTransportBuilder for UTransportBuilderBar {
        fn build(&self) -> Box<dyn UTransport> {
            let utransport_foo: Box<dyn UTransport> = Box::new(UPClientBar::new());
            utransport_foo
        }
    }

    impl UTransportBuilderBar {
        pub fn new() -> Self {
            Self {}
        }
    }

    // #[test]
    // fn test_creating_utransport_router() {
    //     let utransport_builder_foo = UTransportBuilderFoo::new();
    //     let utransport_router_handle =
    //         UTransportRouter::new("FOO".to_string(), utransport_builder_foo);
    //     assert!(utransport_router_handle.is_ok());
    // }

    #[async_std::test]
    async fn test_simple_with_a_single_input_and_output_route() {
        // Local route
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_transport_router =
            UTransportRouter::new("FOO".to_string(), UTransportBuilderFoo::new());
        assert!(local_transport_router.is_ok());
        let local_transport_router_handle = Arc::new(local_transport_router.unwrap());
        let local_route = Route::new(&local_authority, &local_transport_router_handle);

        // A remote route
        let remote_authority = UAuthority {
            name: Some("remote".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_transport_router =
            UTransportRouter::new("BAR".to_string(), UTransportBuilderBar::new());
        assert!(remote_transport_router.is_ok());
        let remote_transport_router_handle = Arc::new(remote_transport_router.unwrap());
        let remote_route = Route::new(&remote_authority, &remote_transport_router_handle);

        let streamer = UStreamer;

        // Add forwarding rules to route local<->remote
        assert_eq!(
            streamer
                .add_forwarding_rule(local_route.clone(), remote_route.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .add_forwarding_rule(remote_route.clone(), local_route.clone())
                .await,
            Ok(())
        );

        // Add forwarding rules to route local<->local, should report an error
        assert!(streamer
            .add_forwarding_rule(local_route.clone(), local_route.clone())
            .await
            .is_err());
        // Try and remove an invalid rule
        assert!(streamer
            .delete_forwarding_rule(remote_route.clone(), remote_route.clone())
            .await
            .is_err());
    }

    //     /**
    //      * This is a simple test where we have a single input and output route.
    //      * We also test the addForwardingRule() and deleteForwardingRule() methods.
    //      */
    //     @Test
    //     @DisplayName("simple test with a single input and output route")
    //     public void simple_test_with_a_single_input_and_output_route() {
    //
    // // Local Route
    // UAuthority localAuthority = UAuthority.newBuilder().setName("local").build();
    // Route local = new Route(localAuthority, new LocalTransport());
    //
    // // A Remote Route
    // UAuthority remoteAuthority = UAuthority.newBuilder().setName("remote").build();
    // Route remote = new Route(remoteAuthority, new RemoteTransport());
    //
    // UStreamer streamer = new UStreamer();
    //
    // // Add forwarding rules to route local<->remote
    // assertEquals(streamer.addForwardingRule(local, remote), UStatus.newBuilder().setCode(UCode.OK).build());
    // assertEquals(streamer.addForwardingRule(remote, local), UStatus.newBuilder().setCode(UCode.OK).build());
    //
    // // Add forwarding rules to route local<->local, should report an error
    // assertEquals(streamer.addForwardingRule(local, local), UStatus.newBuilder().setCode(UCode.INVALID_ARGUMENT).build());
    //
    // // Rule already exists so it should report an error
    // assertEquals(streamer.addForwardingRule(local, remote), UStatus.newBuilder().setCode(UCode.ALREADY_EXISTS).build());
    //
    // // Try and remove an invalid rule
    // assertEquals(streamer.deleteForwardingRule(remote, remote), UStatus.newBuilder().setCode(UCode.INVALID_ARGUMENT).build());
    //
    // // remove valid routing rules
    // assertEquals(streamer.deleteForwardingRule(local, remote), UStatus.newBuilder().setCode(UCode.OK).build());
    // assertEquals(streamer.deleteForwardingRule(remote, local), UStatus.newBuilder().setCode(UCode.OK).build());
    //
    // // Try and remove a rule that doesn't exist, should report an error
    // assertEquals(streamer.deleteForwardingRule(local, remote), UStatus.newBuilder().setCode(UCode.NOT_FOUND).build());
    //
    // }
}
