//! Serialisation helper for the event websocket.

use std::sync::Arc;

use bot_core::events::AppEvent;
use serde_json::{json, Value};

/// Serialise an [`AppEvent`] to JSON, never failing (a serialisation error
/// becomes an error frame rather than dropping the socket).
pub fn event_to_json(event: &Arc<AppEvent>) -> Value {
    serde_json::to_value(event.as_ref()).unwrap_or_else(
        |e| json!({ "kind": "error", "message": format!("event serialization failed: {e}") }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::events::AppEvent;
    use chrono::Utc;

    #[test]
    fn lifecycle_event_serialises_with_kind() {
        let ev = Arc::new(AppEvent::Lifecycle {
            ts: Utc::now(),
            message: "hello".into(),
        });
        let v = event_to_json(&ev);
        assert_eq!(v["kind"], "lifecycle");
        assert_eq!(v["message"], "hello");
    }
}
