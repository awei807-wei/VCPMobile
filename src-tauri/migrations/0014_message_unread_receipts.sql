-- Migration 0014: persist one unread receipt per complete message identity.
--
-- A stream may be delivered more than once (replay, resume, or hydration),
-- and the frontend process may be recreated between deliveries.  The receipt
-- key deliberately contains the complete Wire 1.4 namespace so a reused
-- topic/message id cannot cross-contaminate another owner.
CREATE TABLE message_unread_receipts (
    owner_type TEXT NOT NULL CHECK (owner_type IN ('agent', 'group')),
    owner_id TEXT NOT NULL CHECK (owner_id <> ''),
    topic_id TEXT NOT NULL CHECK (topic_id <> ''),
    msg_id TEXT NOT NULL CHECK (msg_id <> ''),
    created_at BIGINT NOT NULL,
    counted_unread INTEGER NOT NULL DEFAULT 0 CHECK (counted_unread IN (0, 1)),
    PRIMARY KEY (owner_type, owner_id, topic_id, msg_id)
);

CREATE INDEX idx_message_unread_receipts_topic
    ON message_unread_receipts(owner_type, owner_id, topic_id);
