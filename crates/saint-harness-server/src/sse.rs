use crate::AppState;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use saint_harness_protocol::SseWireEvent;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

pub async fn events_all(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let last_id = parse_last_event_id(&headers);
    build_sse_stream(state, None, last_id)
}

pub async fn events_for_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let last_id = parse_last_event_id(&headers);
    build_sse_stream(state, Some(session_id), last_id)
}

fn parse_last_event_id(headers: &HeaderMap) -> i64 {
    headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

fn build_sse_stream(
    state: AppState,
    filter_session: Option<String>,
    last_id: i64,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.store.bus.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |res| match res {
        Ok(env) => {
            if env.stream_id <= last_id {
                return None;
            }
            if let Some(ref target) = filter_session
                && env.event.session_id() != target
            {
                return None;
            }
            let wire = SseWireEvent {
                stream_id: env.stream_id,
                event_type: env.event.event_type_name().to_string(),
                session_id: env.event.session_id().to_string(),
                payload: serde_json::to_value(&env.event).unwrap_or_default(),
            };
            let data = serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_string());
            let sse_ev = Event::default()
                .id(env.stream_id.to_string())
                .event(wire.event_type)
                .data(data);
            Some(Ok(sse_ev))
        }
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
