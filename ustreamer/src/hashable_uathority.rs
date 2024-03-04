use std::hash::{Hash, Hasher};
use prost::bytes::BufMut;
use up_rust::uprotocol::UAuthority;

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
        return false;
    }
}
