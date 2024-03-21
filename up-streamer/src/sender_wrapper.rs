use async_std::channel::Sender;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::Arc;
use up_rust::{UUIDBuilder, UUID};

#[derive(Clone)]
pub(crate) struct SenderWrapper<T> {
    id: UUID,
    sender: Arc<Sender<T>>,
}

impl<T> SenderWrapper<T> {
    pub fn new(sender: Sender<T>) -> Self {
        let id = UUIDBuilder::new().build();
        let sender = Arc::new(sender);
        Self { id, sender }
    }
}

impl<T> Deref for SenderWrapper<T> {
    type Target = Sender<T>;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}

impl<T> Hash for SenderWrapper<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<T> PartialEq for SenderWrapper<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> Eq for SenderWrapper<T> {}
