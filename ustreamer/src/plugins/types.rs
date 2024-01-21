use uprotocol_sdk::uprotocol::{UMessage, UUri};

pub struct QueueEntry {
    source: UUri,
    msg: UMessage,
}
