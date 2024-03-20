use crate::route::Route;

pub struct UStreamer;

impl UStreamer {
    pub async fn add_forwarding_rule(r#in: Route, out: Route) {
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        let forwarding_rule_add_res = out
            .get_transport_router_handle()
            .register(r#in.get_authority(), in_message_sender.clone())
            .await;
        if let Err(e) = forwarding_rule_add_res {
            // log an error here
        }
    }

    pub async fn delete_forwarding_rule(r#in: Route, out: Route) {
        let in_message_sender = &r#in.get_transport_router_handle().clone().message_sender;
        let forwarding_rule_delete_res = out
            .get_transport_router_handle()
            .unregister(r#in.get_authority(), in_message_sender.clone())
            .await;
        if let Err(e) = forwarding_rule_delete_res {
            // log an error here
        }
    }
}
