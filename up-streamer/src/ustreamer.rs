use crate::route::Route;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use up_rust::UStatus;

#[derive(Clone)]
pub struct UStreamer;

impl UStreamer {
    pub async fn add_forwarding_rule(&self, r#in: Route, out: Route) -> Result<(), UStatus> {
        println!("UStreamer::add_forwarding_rule()");
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        println!("r#in.get_authority(): {:#}", &r#in.get_authority());

        // Create a hasher
        let mut hasher = DefaultHasher::new();

        // Hash the instance of SenderWrapper
        in_message_sender.hash(&mut hasher);

        // Obtain the hash
        let hash = hasher.finish();
        println!("in_message_sender hash: {}", hash);

        // ah okay, so I need to include not only the in authority but the out authority

        out.get_transport_router_handle()
            .register(
                r#in.get_authority(),
                out.get_authority(),
                in_message_sender.clone(),
            )
            .await
    }

    pub async fn delete_forwarding_rule(&self, r#in: Route, out: Route) -> Result<(), UStatus> {
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        out.get_transport_router_handle()
            .unregister(
                r#in.get_authority(),
                out.get_authority(),
                in_message_sender.clone(),
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::route::Route;
    #[allow(unused_imports)]
    use crate::ustreamer::UStreamer;
    #[allow(unused_imports)]
    use crate::utransport_builder::UTransportBuilder;
    #[allow(unused_imports)]
    use crate::utransport_router::UTransportRouter;
    #[allow(unused_imports)]
    use async_trait::async_trait;
    #[allow(unused_imports)]
    use std::sync::Arc;
    #[allow(unused_imports)]
    use up_rust::{Number, UAuthority, UMessage, UStatus, UTransport, UUIDBuilder, UUri};

    pub struct UPClientFoo;

    #[async_trait]
    impl UTransport for UPClientFoo {
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            _listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
        ) -> Result<String, UStatus> {
            println!("UPClientFoo: registering topic: {:?}", topic);
            let uuid = UUIDBuilder::new().build();
            Ok(uuid.to_string())
        }

        async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
            println!(
                "UPClientFoo: unregistering topic: {topic:?} with listener string: {listener}"
            );
            Ok(())
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
        async fn send(&self, _message: UMessage) -> Result<(), UStatus> {
            todo!()
        }

        async fn receive(&self, _topic: UUri) -> Result<UMessage, UStatus> {
            todo!()
        }

        async fn register_listener(
            &self,
            topic: UUri,
            _listener: Box<dyn Fn(Result<UMessage, UStatus>) + Send + Sync + 'static>,
        ) -> Result<String, UStatus> {
            println!("UPClientBar: registering topic: {:?}", topic);
            let uuid = UUIDBuilder::new().build();
            Ok(uuid.to_string())
        }

        async fn unregister_listener(&self, topic: UUri, listener: &str) -> Result<(), UStatus> {
            println!(
                "UPClientFoo: unregistering topic: {topic:?} with listener string: {listener}"
            );
            Ok(())
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

    /// This is a simple test where we have a single input and output route.
    /// We also test the add_forwarding_rule() and delete_forwarding_rule() methods.
    #[async_std::test]
    async fn test_simple_with_a_single_input_and_output_route() {
        // Local transport router
        let local_transport_router =
            UTransportRouter::new("FOO".to_string(), UTransportBuilderFoo::new());
        assert!(local_transport_router.is_ok());
        let local_transport_router_handle = Arc::new(local_transport_router.unwrap());

        // Remote transport router
        let remote_transport_router =
            UTransportRouter::new("BAR".to_string(), UTransportBuilderBar::new());
        assert!(remote_transport_router.is_ok());
        let remote_transport_router_handle = Arc::new(remote_transport_router.unwrap());

        // Local route
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_route = Route::new(&local_authority, &local_transport_router_handle);

        // A remote route
        let remote_authority = UAuthority {
            name: Some("remote".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
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

        // Rule already exists so it should report an error
        assert!(streamer
            .add_forwarding_rule(local_route.clone(), remote_route.clone())
            .await
            .is_err());

        // Try and remove an invalid rule
        assert!(streamer
            .delete_forwarding_rule(remote_route.clone(), remote_route.clone())
            .await
            .is_err());

        // remove valid routing rules
        assert_eq!(
            streamer
                .delete_forwarding_rule(local_route.clone(), remote_route.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .delete_forwarding_rule(remote_route.clone(), local_route.clone())
                .await,
            Ok(())
        );

        // Try and remove a rule that doesn't exist, should report an error
        assert!(streamer
            .delete_forwarding_rule(local_route.clone(), remote_route.clone())
            .await
            .is_err());
    }

    /// This is an example where we need to set up multiple routes to different destinations.
    #[async_std::test]
    async fn test_advanced_where_there_is_a_local_route_and_two_remote_routes() {
        // Local transport router
        let local_transport_router =
            UTransportRouter::new("FOO".to_string(), UTransportBuilderFoo::new());
        assert!(local_transport_router.is_ok());
        let local_transport_router_handle = Arc::new(local_transport_router.unwrap());

        // First remote transport router
        let remote_transport_router_1 =
            UTransportRouter::new("BAR".to_string(), UTransportBuilderBar::new());
        assert!(remote_transport_router_1.is_ok());
        let remote_transport_router_handle_1 = Arc::new(remote_transport_router_1.unwrap());

        // Second remote transport router
        let remote_transport_router_2 =
            UTransportRouter::new("BAR".to_string(), UTransportBuilderBar::new());
        assert!(remote_transport_router_2.is_ok());
        let remote_transport_router_handle_2 = Arc::new(remote_transport_router_2.unwrap());

        // Local route
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_route = Route::new(&local_authority, &local_transport_router_handle);

        // A first remote route
        let remote_authority_1 = UAuthority {
            name: Some("remote_1".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_route_1 = Route::new(&remote_authority_1, &remote_transport_router_handle_1);

        // A second remote route
        let remote_authority_2 = UAuthority {
            name: Some("remote_2".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 201])),
            ..Default::default()
        };
        let remote_route_2 = Route::new(&remote_authority_2, &remote_transport_router_handle_2);

        let streamer = UStreamer;

        // Add forwarding rules to route local_route<->remote_route_1
        assert_eq!(
            streamer
                .add_forwarding_rule(local_route.clone(), remote_route_1.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .add_forwarding_rule(remote_route_1.clone(), local_route.clone())
                .await,
            Ok(())
        );

        // Add forwarding rules to route local_route<->remote_route_2
        assert_eq!(
            streamer
                .add_forwarding_rule(local_route.clone(), remote_route_2.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .add_forwarding_rule(remote_route_2.clone(), local_route.clone())
                .await,
            Ok(())
        );
    }

    /// This is an example where we need to set up multiple routes to different destinations but using the same
    /// remote UTransport (i.e. connecting to multiple remote servers using the same UTransport instance).
    #[async_std::test]
    async fn test_advanced_where_there_is_an_local_route_and_two_remote_routes_but_the_remote_routes_have_the_same_instance_of_utransport(
    ) {
        // Local transport router
        let local_transport_router =
            UTransportRouter::new("FOO".to_string(), UTransportBuilderFoo::new());
        assert!(local_transport_router.is_ok());
        let local_transport_router_handle = Arc::new(local_transport_router.unwrap());

        // Remote transport router
        let remote_transport_router =
            UTransportRouter::new("BAR".to_string(), UTransportBuilderBar::new());
        assert!(remote_transport_router.is_ok());
        let remote_transport_router_handle = Arc::new(remote_transport_router.unwrap());

        // Local route
        let local_authority = UAuthority {
            name: Some("local".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 100])),
            ..Default::default()
        };
        let local_route = Route::new(&local_authority, &local_transport_router_handle);

        // A first remote route
        let remote_authority_1 = UAuthority {
            name: Some("remote_1".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 200])),
            ..Default::default()
        };
        let remote_route_1 = Route::new(&remote_authority_1, &remote_transport_router_handle);

        // A second remote route
        let remote_authority_2 = UAuthority {
            name: Some("remote_2".to_string()),
            number: Some(Number::Ip(vec![192, 168, 1, 201])),
            ..Default::default()
        };
        let remote_route_2 = Route::new(&remote_authority_2, &remote_transport_router_handle);

        let streamer = UStreamer;

        // Add forwarding rules to route local_route<->remote_route_1
        assert_eq!(
            streamer
                .add_forwarding_rule(local_route.clone(), remote_route_1.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .add_forwarding_rule(remote_route_1.clone(), local_route.clone())
                .await,
            Ok(())
        );

        // Add forwarding rules to route local_route<->remote_route_2
        assert_eq!(
            streamer
                .add_forwarding_rule(local_route.clone(), remote_route_2.clone())
                .await,
            Ok(())
        );
        assert_eq!(
            streamer
                .add_forwarding_rule(remote_route_2.clone(), local_route.clone())
                .await,
            Ok(())
        );
    }

    /*
    /// This is an example where we need to set up multiple routes to different destinations where one of the
    /// routes is the default route (ex. the cloud gateway)
    ///
    /// TODO: Visit with Steven on this point
    ///  We'd have to have another ability within register_listener to register on wildcard authority
     */
}
