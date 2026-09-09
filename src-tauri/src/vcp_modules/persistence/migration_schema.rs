#[path = "migration_schema_inspect.rs"]
mod inspect;
pub(super) use inspect::inspect_legacy_schema;

#[derive(Debug, Default)]
pub(super) struct LegacySchema {
    attachment_deleted_at: bool,
    messages_fts: bool,
    fts_topic_identity: bool,
    active_generations: bool,
    render_content_hash: bool,
    render_schema_version: bool,
    avatar_deleted_at: bool,
    composite_topics: bool,
    composite_messages: bool,
    composite_render_cache: bool,
    composite_message_attachments: bool,
    composite_active_generations: bool,
    topic_activity_clock: bool,
    fts_composite_identity: bool,
    group_member_tags_table: bool,
    group_member_tags: bool,
    helper_generation: bool,
    recovery_cleanup_outbox_table: bool,
    recovery_cleanup_outbox: bool,
    attachment_gc_unlink_outbox_table: bool,
    attachment_gc_unlink_outbox: bool,
    attachment_gc_unlink_live_references_table: bool,
    attachment_gc_unlink_live_references: bool,
    attachment_gc_internal_path_index: bool,
    attachment_gc_thumbnail_path_index: bool,
    attachment_gc_hash_index: bool,
    attachment_gc_message_hash_index: bool,
    message_unread_receipts_table: bool,
    message_unread_receipts: bool,
}

impl LegacySchema {
    pub(super) fn validate_partial_states(&self) -> Result<(), String> {
        self.validate_render_cache_state()?;
        self.validate_composite_state()?;
        self.validate_extension_states()?;
        self.validate_reference_index_state()
    }

    fn validate_render_cache_state(&self) -> Result<(), String> {
        if self.render_content_hash != self.render_schema_version {
            return Err(
                "Bootstrap: render_cache identity migration is only partially applied; database left unchanged"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn validate_composite_state(&self) -> Result<(), String> {
        let composite_states = [
            self.composite_topics,
            self.composite_messages,
            self.composite_render_cache,
            self.composite_message_attachments,
            self.composite_active_generations,
            self.topic_activity_clock,
            self.fts_composite_identity,
        ];
        if composite_states.iter().any(|state| *state)
            && !composite_states.iter().all(|state| *state)
        {
            return Err(
                "Bootstrap: Wire 1.4 composite identity migration is only partially applied; database left unchanged"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn validate_extension_states(&self) -> Result<(), String> {
        validate_partial_pair(
            self.group_member_tags_table,
            self.group_member_tags,
            "Bootstrap: group_member_tags migration is only partially applied; database left unchanged",
        )?;
        validate_partial_pair(
            self.recovery_cleanup_outbox_table,
            self.recovery_cleanup_outbox,
            "Bootstrap: recovery cleanup outbox migration is only partially applied; database left unchanged",
        )?;
        validate_partial_pair(
            self.attachment_gc_unlink_outbox_table,
            self.attachment_gc_unlink_outbox,
            "Bootstrap: attachment GC unlink outbox migration is only partially applied; database left unchanged",
        )?;
        validate_partial_pair(
            self.attachment_gc_unlink_live_references_table,
            self.attachment_gc_unlink_live_references,
            "Bootstrap: attachment GC live-reference witness migration is only partially applied; database left unchanged",
        )?;
        validate_partial_pair(
            self.message_unread_receipts_table,
            self.message_unread_receipts,
            "Bootstrap: message unread receipts migration is only partially applied; database left unchanged",
        )
    }

    fn validate_reference_index_state(&self) -> Result<(), String> {
        let attachment_gc_reference_indexes = [
            self.attachment_gc_internal_path_index,
            self.attachment_gc_thumbnail_path_index,
            self.attachment_gc_hash_index,
            self.attachment_gc_message_hash_index,
        ];
        if attachment_gc_reference_indexes.iter().any(|state| *state)
            && !attachment_gc_reference_indexes.iter().all(|state| *state)
        {
            return Err(
                "Bootstrap: bounded attachment GC reference-index migration is only partially applied; database left unchanged"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn has_composite_identity(&self) -> bool {
        self.composite_topics
            && self.composite_messages
            && self.composite_render_cache
            && self.composite_message_attachments
            && self.composite_active_generations
            && self.topic_activity_clock
            && self.fts_composite_identity
    }

    fn merge(&mut self, other: Self) {
        self.attachment_deleted_at |= other.attachment_deleted_at;
        self.messages_fts |= other.messages_fts;
        self.fts_topic_identity |= other.fts_topic_identity;
        self.active_generations |= other.active_generations;
        self.render_content_hash |= other.render_content_hash;
        self.render_schema_version |= other.render_schema_version;
        self.avatar_deleted_at |= other.avatar_deleted_at;
        self.composite_topics |= other.composite_topics;
        self.composite_messages |= other.composite_messages;
        self.composite_render_cache |= other.composite_render_cache;
        self.composite_message_attachments |= other.composite_message_attachments;
        self.composite_active_generations |= other.composite_active_generations;
        self.topic_activity_clock |= other.topic_activity_clock;
        self.fts_composite_identity |= other.fts_composite_identity;
        self.group_member_tags_table |= other.group_member_tags_table;
        self.group_member_tags |= other.group_member_tags;
        self.helper_generation |= other.helper_generation;
        self.recovery_cleanup_outbox_table |= other.recovery_cleanup_outbox_table;
        self.recovery_cleanup_outbox |= other.recovery_cleanup_outbox;
        self.attachment_gc_unlink_outbox_table |= other.attachment_gc_unlink_outbox_table;
        self.attachment_gc_unlink_outbox |= other.attachment_gc_unlink_outbox;
        self.attachment_gc_unlink_live_references_table |=
            other.attachment_gc_unlink_live_references_table;
        self.attachment_gc_unlink_live_references |= other.attachment_gc_unlink_live_references;
        self.attachment_gc_internal_path_index |= other.attachment_gc_internal_path_index;
        self.attachment_gc_thumbnail_path_index |= other.attachment_gc_thumbnail_path_index;
        self.attachment_gc_hash_index |= other.attachment_gc_hash_index;
        self.attachment_gc_message_hash_index |= other.attachment_gc_message_hash_index;
        self.message_unread_receipts_table |= other.message_unread_receipts_table;
        self.message_unread_receipts |= other.message_unread_receipts;
    }

    pub(super) fn proves_migration(&self, version: i64) -> bool {
        match version {
            1 => true,
            2 => self.attachment_deleted_at,
            3 => self.messages_fts,
            4 => self.fts_topic_identity,
            5 => self.active_generations,
            6 => self.render_content_hash && self.render_schema_version,
            7 => self.avatar_deleted_at,
            8 => self.has_composite_identity(),
            9 => self.group_member_tags,
            10 => self.helper_generation,
            11 => self.recovery_cleanup_outbox,
            12 => self.attachment_gc_unlink_outbox,
            13 => {
                self.attachment_gc_internal_path_index
                    && self.attachment_gc_thumbnail_path_index
                    && self.attachment_gc_hash_index
                    && self.attachment_gc_message_hash_index
            }
            14 => self.message_unread_receipts,
            15 => self.attachment_gc_unlink_live_references,
            _ => false,
        }
    }

    pub(super) fn tracked_migration_is_consistent(&self, version: i64) -> bool {
        match version {
            1..=15 => self.proves_migration(version),
            _ => true,
        }
    }
}

fn validate_partial_pair(
    table_exists: bool,
    schema_matches: bool,
    message: &str,
) -> Result<(), String> {
    if table_exists && !schema_matches {
        return Err(message.to_string());
    }
    Ok(())
}
