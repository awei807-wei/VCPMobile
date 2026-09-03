use crate::vcp_modules::chat::topic_types::{MessageKey, OwnerKey, TopicKey};
use dashmap::DashMap;
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::sync::oneshot;

/// A request entry is scoped by the complete message identity.
///
/// The epoch prevents an older stream guard from removing a newer request that
/// reused the same message id after a retry or process recovery.
struct ActiveRequestEntry {
    sender: oneshot::Sender<()>,
    epoch: u64,
}

/// Thread-safe registry for cancellable VCP requests.
pub struct ActiveRequestRegistry {
    entries: DashMap<MessageKey, ActiveRequestEntry>,
    next_epoch: AtomicU64,
}

impl Default for ActiveRequestRegistry {
    fn default() -> Self {
        Self {
            entries: DashMap::new(),
            next_epoch: AtomicU64::new(0),
        }
    }
}

impl ActiveRequestRegistry {
    /// Register an exact request identity and return its epoch plus any prior
    /// sender that should be cancelled by the caller.
    pub fn register(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
    ) -> (u64, Option<oneshot::Sender<()>>) {
        let epoch = self
            .next_epoch
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let previous = self
            .entries
            .insert(key, ActiveRequestEntry { sender, epoch });
        (epoch, previous.map(|entry| entry.sender))
    }

    /// Compatibility insertion API for internal callers that do not need the
    /// epoch. New stream code should use [`Self::register`].
    pub fn insert(
        &self,
        key: MessageKey,
        sender: oneshot::Sender<()>,
    ) -> Option<oneshot::Sender<()>> {
        self.register(key, sender).1
    }

    /// Remove an exact key and return the key with its cancellation sender.
    pub fn remove_key(&self, key: &MessageKey) -> Option<(MessageKey, oneshot::Sender<()>)> {
        self.entries
            .remove(key)
            .map(|(key, entry)| (key, entry.sender))
    }

    /// Remove only if the entry still belongs to the supplied registration.
    /// This is the guard path used when a stream exits.
    pub fn remove_if_current(&self, key: &MessageKey, epoch: u64) -> bool {
        self.entries
            .remove_if(key, |_key, entry| entry.epoch == epoch)
            .is_some()
    }

    /// Remove by an exact composite key, or by a legacy message id only when
    /// exactly one active request has that id. Ambiguous legacy lookup fails
    /// closed and returns `None`.
    pub fn remove<Q>(&self, query: &Q) -> Option<(MessageKey, oneshot::Sender<()>)>
    where
        Q: ActiveRequestQuery + ?Sized,
    {
        query.resolve(self).and_then(|key| self.remove_key(&key))
    }

    /// Check an exact key, or an unambiguous legacy message id.
    pub fn contains_key<Q>(&self, query: &Q) -> bool
    where
        Q: ActiveRequestQuery + ?Sized,
    {
        query
            .resolve(self)
            .is_some_and(|key| self.entries.contains_key(&key))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn resolve_legacy_message_id(&self, message_id: &str) -> Option<MessageKey> {
        let mut found = None;
        for entry in self.entries.iter() {
            if entry.key().msg_id != message_id {
                continue;
            }
            if found.is_some() {
                // Never guess an owner/topic when the old API only supplied
                // msgId. This is the fail-closed compatibility boundary.
                return None;
            }
            found = Some(entry.key().clone());
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

/// Public state wrapper retained for Tauri and existing call sites.
pub struct ActiveRequests(pub Arc<ActiveRequestRegistry>);

impl Default for ActiveRequests {
    fn default() -> Self {
        log::info!("[VCPClient] Initialized ActiveRequests successfully.");
        Self(Arc::new(ActiveRequestRegistry::default()))
    }
}

/// Build a complete message identity from the request context.
///
/// Current agent/group callers carry `agentId`/`groupId` and `topicId`. An
/// explicit `ownerType` is honored when present; incomplete or conflicting
/// identity is rejected before any request is registered.
pub fn message_key_from_context(
    context: Option<&Value>,
    message_id: &str,
) -> Result<MessageKey, String> {
    let context = context.ok_or_else(|| {
        "VCP request requires ownerType/ownerId and topicId for composite identity".to_string()
    })?;
    let object = context.as_object().ok_or_else(|| {
        "VCP request context must be an object with composite owner identity".to_string()
    })?;

    let group_id = object.get("groupId").and_then(Value::as_str);
    let agent_id = object.get("agentId").and_then(Value::as_str);
    let explicit_owner_type = object.get("ownerType").and_then(Value::as_str);
    let owner_type =
        explicit_owner_type.unwrap_or(if group_id.is_some() { "group" } else { "agent" });
    let owner_id = match owner_type {
        "group" => group_id.or_else(|| object.get("ownerId").and_then(Value::as_str)),
        "agent" => agent_id.or_else(|| object.get("ownerId").and_then(Value::as_str)),
        _ => None,
    }
    .ok_or_else(|| format!("VCP request has no valid {owner_type} owner id"))?;
    let topic_id = object
        .get("topicId")
        .and_then(Value::as_str)
        .ok_or_else(|| "VCP request has no topicId".to_string())?;

    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    if !key.is_valid() {
        return Err(format!(
            "VCP request has invalid composite identity: {}/{}/{}/{}",
            key.topic.owner_type, key.topic.owner_id, key.topic.topic_id, key.msg_id
        ));
    }
    Ok(key)
}

/// Build a complete message identity from explicit command arguments.
pub fn message_key_from_parts(
    owner_id: &str,
    owner_type: &str,
    topic_id: &str,
    message_id: &str,
) -> Result<MessageKey, String> {
    let key = MessageKey::new(TopicKey::new(owner_type, owner_id, topic_id), message_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err(format!(
            "invalid composite identity: {owner_type}/{owner_id}/{topic_id}/{message_id}"
        ))
    }
}

/// Resolve an owner identity from optional legacy command arguments.
/// Supplying only part of the identity is rejected to avoid accidental
/// cross-owner operations.
pub fn optional_message_key(
    message_id: &str,
    owner_id: Option<String>,
    owner_type: Option<String>,
    topic_id: Option<String>,
) -> Result<Option<MessageKey>, String> {
    match (owner_id, owner_type, topic_id) {
        (None, None, None) => Ok(None),
        (Some(owner_id), Some(owner_type), Some(topic_id)) => {
            message_key_from_parts(&owner_id, &owner_type, &topic_id, message_id).map(Some)
        }
        _ => Err(
            "ownerId, ownerType and topicId must be supplied together for composite identity"
                .to_string(),
        ),
    }
}

/// Stable owner helper for callers that need to produce context values.
pub fn owner_key_from_parts(owner_id: &str, owner_type: &str) -> Result<OwnerKey, String> {
    let key = OwnerKey::new(owner_type, owner_id);
    if key.is_valid() {
        Ok(key)
    } else {
        Err(format!("invalid owner identity: {owner_type}/{owner_id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(owner_type: &str, owner_id: &str, topic_id: &str, msg_id: &str) -> MessageKey {
        message_key_from_parts(owner_id, owner_type, topic_id, msg_id).unwrap()
    }

    #[test]
    fn active_requests_keep_same_message_id_isolated_by_composite_key() {
        let registry = ActiveRequestRegistry::default();
        let agent_key = key("agent", "owner-a", "shared-topic", "same-msg");
        let group_key = key("group", "owner-g", "shared-topic", "same-msg");
        let (agent_tx, _agent_rx) = oneshot::channel();
        let (group_tx, _group_rx) = oneshot::channel();

        registry.register(agent_key.clone(), agent_tx);
        registry.register(group_key.clone(), group_tx);

        assert!(registry.contains_key(&agent_key));
        assert!(registry.contains_key(&group_key));
        assert!(!registry.contains_key("same-msg"));
        assert!(registry.remove_key(&agent_key).is_some());
        assert!(!registry.contains_key(&agent_key));
        assert!(registry.contains_key(&group_key));
        assert!(registry.remove_key(&group_key).is_some());
        assert!(registry.is_empty());
    }

    #[test]
    fn legacy_remove_is_fail_closed_when_message_id_is_ambiguous() {
        let registry = ActiveRequestRegistry::default();
        registry.register(
            key("agent", "owner-a", "topic", "same-msg"),
            oneshot::channel().0,
        );
        registry.register(
            key("group", "owner-g", "topic", "same-msg"),
            oneshot::channel().0,
        );

        assert!(registry.remove("same-msg").is_none());
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn stale_epoch_cannot_remove_newer_registration() {
        let registry = ActiveRequestRegistry::default();
        let key = key("agent", "owner-a", "topic", "same-msg");
        let (old_tx, _old_rx) = oneshot::channel();
        let (old_epoch, _) = registry.register(key.clone(), old_tx);
        let (new_tx, _new_rx) = oneshot::channel();
        let (new_epoch, old_sender) = registry.register(key.clone(), new_tx);
        assert!(old_sender.is_some());
        assert_ne!(old_epoch, new_epoch);
        assert!(!registry.remove_if_current(&key, old_epoch));
        assert!(registry.contains_key(&key));
        assert!(registry.remove_if_current(&key, new_epoch));
        assert!(registry.is_empty());
    }

    #[test]
    fn context_requires_complete_owner_topic_message_identity() {
        let context = serde_json::json!({
            "groupId": "owner-g",
            "agentId": "agent-a",
            "topicId": "shared-topic"
        });
        assert_eq!(
            message_key_from_context(Some(&context), "same-msg").unwrap(),
            key("group", "owner-g", "shared-topic", "same-msg")
        );
        assert!(message_key_from_context(None, "same-msg").is_err());
        assert!(message_key_from_context(
            Some(&serde_json::json!({"agentId": "owner-a"})),
            "same-msg"
        )
        .is_err());
    }
}
