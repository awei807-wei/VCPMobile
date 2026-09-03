use super::{StreamControl, StreamSession};
use crate::vcp_modules::aurora_pipeline::AuroraUpdate;
use crate::vcp_modules::infra::vcp_client::StreamEvent;
use serde_json::json;
use std::time::Instant;
use tauri::Runtime;

fn adaptive_parse_interval_ms(tail_len: usize) -> u128 {
    match tail_len {
        0..=8_191 => 33,
        8_192..=24_575 => 100,
        _ => 200,
    }
}

fn adaptive_force_bytes(tail_len: usize) -> usize {
    match tail_len {
        0..=8_191 => 1024,
        8_192..=24_575 => 4096,
        _ => 8192,
    }
}

impl<R: Runtime> StreamSession<R> {
    pub(super) fn initialize_content(&mut self) {
        let Some(content) = self.initial_content.take() else {
            return;
        };
        self.aurora_buffer.append_chunk(&content);
        let _ = self.aurora_buffer.process_queue();
        self.aurora_buffer.pushed_len = content.len();
        let _ = self.aurora_buffer.take_chunk();
        let _ = self.aurora_buffer.take_tail_frame();
    }

    pub(super) fn flush_aurora_parse(&mut self, force: bool) -> (bool, bool) {
        if self.pending_aurora_chunk.is_empty() {
            return (false, false);
        }
        let projected_tail_len =
            self.aurora_buffer.tail_content.len() + self.pending_aurora_chunk.len();
        let due_by_time = self.last_aurora_parse.elapsed().as_millis()
            >= adaptive_parse_interval_ms(projected_tail_len);
        let due_by_size =
            self.pending_aurora_chunk.len() >= adaptive_force_bytes(projected_tail_len);
        if !force && !due_by_time && !due_by_size {
            return (false, false);
        }
        self.aurora_buffer.append_chunk(&self.pending_aurora_chunk);
        self.pending_aurora_chunk.clear();
        self.last_aurora_parse = Instant::now();
        self.aurora_buffer.process_queue()
    }

    pub(super) fn send_aurora_update(
        &mut self,
        stable_changed: bool,
        tail_changed: bool,
        finish_reason: Option<String>,
        error: Option<String>,
    ) {
        let is_final = finish_reason.is_some() || error.is_some();
        let chunk = self.aurora_buffer.take_chunk();
        let tail_frame = self.aurora_buffer.take_tail_frame();
        let tail_snapshot = tail_frame.as_ref().and_then(|frame| frame.snapshot.clone());
        let mut event = StreamEvent::aurora(
            self.message_id.clone(),
            AuroraUpdate {
                stable_blocks: stable_changed.then(|| self.aurora_buffer.stable_blocks.clone()),
                stable_changed,
                tail_block: tail_changed
                    .then(|| self.aurora_buffer.tail_block.clone())
                    .flatten(),
                tail: tail_changed.then(|| self.aurora_buffer.tail_content.clone()),
                tail_changed,
                tail_frame,
                tail_snapshot,
                content: is_final.then(|| self.aurora_buffer.full_text.clone()),
                chunk,
            },
            self.context.clone(),
        );
        event.finish_reason = finish_reason;
        event.error = error;
        self.send_stream_event(event);
    }

    pub(super) fn finish_success(&mut self) -> StreamControl {
        let _ = self.flush_aurora_parse(true);
        self.aurora_buffer.finalize();
        self.send_aurora_update(true, true, self.last_finish_reason.clone(), None);
        self.remove_active_request();
        StreamControl::Complete(Ok((
            json!({
                "fullContent": self.aurora_buffer.full_text,
                "streamingStarted": true,
                "finishReason": self.last_finish_reason
            }),
            false,
        )))
    }

    pub(super) fn cancel_before_streaming(&mut self) -> StreamControl {
        let _ = self.flush_aurora_parse(true);
        self.aurora_buffer.finalize();
        self.send_aurora_update(
            true,
            true,
            Some("cancelled_by_user".to_string()),
            Some("请求已中止".to_string()),
        );
        self.remove_active_request();
        StreamControl::Complete(Ok((
            json!({
                "fullContent": self.aurora_buffer.full_text,
                "streamingStarted": false
            }),
            true,
        )))
    }

    pub(super) fn cancel_during_stream(&mut self) -> StreamControl {
        let _ = self.flush_aurora_parse(true);
        self.aurora_buffer.finalize();
        self.send_aurora_update(
            true,
            true,
            Some("cancelled_by_user".to_string()),
            Some("请求已中止".to_string()),
        );
        self.remove_active_request();
        StreamControl::Complete(Ok((
            json!({
                "fullContent": self.aurora_buffer.full_text,
                "finishReason": "cancelled_by_user"
            }),
            true,
        )))
    }

    pub(super) fn cancel_silently(&mut self) -> StreamControl {
        self.remove_active_request();
        StreamControl::Complete(Ok((
            json!({
                "fullContent": self.aurora_buffer.full_text,
                "finishReason": "cancelled_by_user"
            }),
            true,
        )))
    }

    pub(super) fn fail_with_error(&mut self, error: String) -> StreamControl {
        self.send_stream_event(StreamEvent::error(
            self.message_id.clone(),
            self.context.clone(),
            error.clone(),
        ));
        self.remove_active_request();
        StreamControl::Complete(Err(error))
    }
}
