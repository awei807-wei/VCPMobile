use super::{ActiveRequestRegistry, MessageKey};

impl ActiveRequestRegistry {
    pub(super) fn resolve_legacy_message_id(&self, message_id: &str) -> Option<MessageKey> {
        let mut found = None;
        for key in self
            .inner
            .current_epochs
            .iter()
            .map(|entry| entry.key().clone())
            .chain(
                self.inner
                    .entries
                    .iter()
                    .filter(|entry| !self.inner.current_epochs.contains_key(entry.key()))
                    .map(|entry| entry.key().clone()),
            )
        {
            if key.msg_id != message_id {
                continue;
            }
            if found.is_some() {
                // Never guess an owner/topic when the old API only supplied
                // msgId. This is the fail-closed compatibility boundary.
                return None;
            }
            found = Some(key);
        }
        found
    }
}

/// Query types accepted by compatibility methods on [`ActiveRequestRegistry`].
pub trait ActiveRequestQuery {
    fn resolve(&self, registry: &ActiveRequestRegistry) -> Option<MessageKey>;
}

impl ActiveRequestQuery for MessageKey {
    fn resolve(&self, _registry: &ActiveRequestRegistry) -> Option<MessageKey> {
        Some(self.clone())
    }
}

impl ActiveRequestQuery for String {
    fn resolve(&self, registry: &ActiveRequestRegistry) -> Option<MessageKey> {
        registry.resolve_legacy_message_id(self)
    }
}

impl ActiveRequestQuery for str {
    fn resolve(&self, registry: &ActiveRequestRegistry) -> Option<MessageKey> {
        registry.resolve_legacy_message_id(self)
    }
}
