//! Best-effort live delivery. Revision/sequence gaps are recovered with readThread,
//! never by replaying a mutation or presenting an old snapshot as current state.
use serde_json::{json, Value};
use std::{
    sync::mpsc::{self, SyncSender},
    time::Duration,
};
pub(crate) struct Publisher(SyncSender<Value>);
impl Publisher {
    pub(crate) fn from_env() -> Option<Self> {
        let app = std::env::var("BAIJIMU_LOCAL_APP_ID").ok()?;
        let endpoint = std::env::var("BAIJIMU_LOCAL_APP_EVENT_ENDPOINT").ok()?;
        let token_path = std::env::var_os("BAIJIMU_LOCAL_APP_EVENT_TOKEN_FILE")?;
        let token = std::fs::read_to_string(token_path).ok()?.trim().to_string();
        if token.is_empty() {
            return None;
        }
        let (tx, rx) = mpsc::sync_channel::<Value>(8);
        std::thread::spawn(move || {
            let Ok(client) = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
            else {
                return;
            };
            while let Ok(payload) = rx.recv() {
                let event = json!({"appId":app,"event":"codexDesktopEvent","eventId":crate::random_event_id(),"occurredAt":payload["receivedAt"],"payload":payload});
                match client
                    .post(&endpoint)
                    .bearer_auth(&token)
                    .json(&event)
                    .send()
                {
                    Ok(r) if r.status().is_success() => {}
                    _ => eprintln!(
                        "desktop event delivery failed; consumer must resynchronize by revision"
                    ),
                }
            }
        });
        Some(Self(tx))
    }
    pub(crate) fn publish(&self, event: Value) {
        if self.0.try_send(event).is_err() {
            eprintln!("desktop event queue full; consumer must resynchronize by revision");
        }
    }
}
