use crate::{
    connector_home, events, random_event_id, thread_state, timestamp,
    DOMAIN_EVENT_PUBLISH_ATTEMPTS, DOMAIN_EVENT_RETRY_BASE_DELAY, MAX_EVENTS,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread;

const EVENT_PUBLISH_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Debug)]
struct ConnectorEvent {
    sequence: u64,
    received_at: String,
    method: String,
    params: Value,
}

#[derive(Default)]
struct EventState {
    sequence: u64,
    retained: VecDeque<ConnectorEvent>,
}

pub(super) struct EventStore {
    state: Mutex<EventState>,
    publisher: Option<EventPublisher>,
    stream_id: String,
}
struct EventPublisher {
    sender: SyncSender<PublishJob>,
}

struct PublishJob {
    event_name: &'static str,
    event_id: String,
    occurred_at: String,
    payload: Value,
    attempts: usize,
}

struct PublisherWorker {
    app_id: String,
    endpoint: String,
    token: String,
    client: reqwest::blocking::Client,
}
impl EventStore {
    pub(super) fn push(&self, method: &str, params: Value) {
        if method == "turn/completed" {
            if let Some(thread_id) = params.get("threadId").and_then(Value::as_str) {
                if let Err(error) = thread_state::mark_thread_unread(&connector_home(), thread_id) {
                    eprintln!("failed to persist unread Codex thread {thread_id}: {error}");
                }
            }
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.sequence += 1;
        let sequence = state.sequence;
        let received_at = timestamp();
        let domain_event = events::normalize_codex_notification(
            method,
            &params,
            &received_at,
            &self.stream_id,
            sequence,
        )
        .map_err(|error| {
            eprintln!("failed to normalize Codex domain event {method}: {error}");
            error
        })
        .ok()
        .flatten();
        let event = ConnectorEvent {
            sequence,
            received_at,
            method: method.to_string(),
            params,
        };
        state.retained.push_back(event.clone());
        while state.retained.len() > MAX_EVENTS {
            state.retained.pop_front();
        }
        drop(state);
        if let Some(publisher) = &self.publisher {
            publisher.publish_raw(event);
            if let Some(domain_event) = domain_event {
                publisher.publish_domain(domain_event);
            }
        }
    }

    pub(super) fn recent(&self, body: &Value) -> Value {
        let after_sequence = body
            .get("afterSequence")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let limit = body
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(100)
            .clamp(1, 500) as usize;
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let events = state
            .retained
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|event| {
                json!({
                    "sequence": event.sequence,
                    "receivedAt": event.received_at,
                    "method": event.method,
                    "params": event.params,
                })
            })
            .collect::<Vec<_>>();
        json!({"latestSequence": state.sequence, "events": events})
    }

    pub(super) fn summary(&self) -> (u64, usize) {
        self.state
            .lock()
            .map(|state| (state.sequence, state.retained.len()))
            .unwrap_or((0, 0))
    }
}

impl EventPublisher {
    fn from_env() -> Option<Self> {
        let app_id = env::var("BAIJIMU_LOCAL_APP_ID").ok()?;
        let endpoint = env::var("BAIJIMU_LOCAL_APP_EVENT_ENDPOINT").ok()?;
        let token_path = env::var("BAIJIMU_LOCAL_APP_EVENT_TOKEN_FILE").ok()?;
        let token = fs::read_to_string(token_path).ok()?.trim().to_string();
        if endpoint.trim().is_empty() || token.is_empty() {
            return None;
        }
        let (sender, receiver) = mpsc::sync_channel(EVENT_PUBLISH_QUEUE_CAPACITY);
        let worker = PublisherWorker {
            app_id,
            endpoint,
            token,
            client: reqwest::blocking::Client::new(),
        };
        thread::spawn(move || worker.run(receiver));
        Some(Self { sender })
    }

    fn publish_raw(&self, event: ConnectorEvent) {
        self.enqueue(PublishJob {
            event_name: "codexNotification",
            event_id: random_event_id(),
            occurred_at: event.received_at.clone(),
            payload: json!({
                "sequence": event.sequence,
                "receivedAt": event.received_at,
                "method": event.method,
                "params": event.params,
            }),
            attempts: 1,
        });
    }

    fn publish_domain(&self, event: events::DomainEvent) {
        self.enqueue(PublishJob {
            event_name: event.name,
            event_id: event.event_id,
            occurred_at: event.occurred_at,
            payload: event.payload,
            attempts: DOMAIN_EVENT_PUBLISH_ATTEMPTS,
        });
    }

    fn enqueue(&self, job: PublishJob) {
        let event_name = job.event_name;
        match self.sender.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                eprintln!("event publish queue is full; dropped {event_name}")
            }
            Err(TrySendError::Disconnected(_)) => {
                eprintln!("event publisher stopped; dropped {event_name}")
            }
        }
    }
}

impl PublisherWorker {
    fn run(self, receiver: Receiver<PublishJob>) {
        while let Ok(job) = receiver.recv() {
            self.publish(job);
        }
    }

    fn publish(&self, job: PublishJob) {
        let request = json!({
            "appId": self.app_id,
            "event": job.event_name,
            "eventId": job.event_id,
            "payload": job.payload,
            "occurredAt": job.occurred_at,
        });
        let attempts = job.attempts.max(1);
        for attempt in 1..=attempts {
            match self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.token)
                .json(&request)
                .send()
            {
                Ok(response) if response.status().is_success() => return,
                Ok(response) if !retryable_event_status(response.status().as_u16()) => {
                    eprintln!(
                        "failed to publish {}: bridge returned HTTP {}",
                        job.event_name,
                        response.status()
                    );
                    return;
                }
                Ok(response) if attempt == attempts => eprintln!(
                    "failed to publish {} after {attempts} attempts: bridge returned HTTP {}",
                    job.event_name,
                    response.status()
                ),
                Err(error) if attempt == attempts => eprintln!(
                    "failed to publish {} after {attempts} attempts: {error}",
                    job.event_name
                ),
                Ok(_) | Err(_) => {}
            }
            if attempt < attempts {
                let multiplier = 1_u32 << (attempt - 1).min(8);
                thread::sleep(DOMAIN_EVENT_RETRY_BASE_DELAY * multiplier);
            }
        }
    }
}

pub(crate) fn retryable_event_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

impl EventStore {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(EventState::default()),
            publisher: EventPublisher::from_env(),
            stream_id: random_event_id(),
        }
    }
}
