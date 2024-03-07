use prost::bytes::BufMut;
use std::hash::{Hash, Hasher};
use up_rust::uprotocol::{UAuthority, UUID};

// TODO: Consider how much of this to upstream into `up-rust`

#[derive(Clone)]
pub struct HashableUAuthority(pub UAuthority);

impl PartialEq for HashableUAuthority {
    fn eq(&self, other: &HashableUAuthority) -> bool {
        self.0 == other.0
    }
}

impl Eq for HashableUAuthority {}

impl Hash for HashableUAuthority {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut bytes = Vec::new();
        if self.0.has_id() {
            bytes.put_u8(self.0.id().len() as u8);
            bytes.put(self.0.id());
        } else if self.0.has_ip() {
            bytes.put(self.0.ip());
        } else {
            // Should never happen, call hashable first!
            bytes.put_u8(42);
        }

        bytes.hash(state)
    }
}
impl HashableUAuthority {
    pub(crate) fn hashable(&self) -> bool {
        if self.0.has_id() || self.0.has_ip() {
            return true;
        }
        false
    }
}

pub struct HashableUUID(pub UUID);

impl PartialEq for HashableUUID {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for HashableUUID {}

impl Hash for HashableUUID {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}
