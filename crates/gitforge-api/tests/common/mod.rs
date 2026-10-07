//! Test doubles for the CI trigger delegation seam, shared by the
//! gitforge-api integration suites.

use async_trait::async_trait;
use gitforge_api::routes::CiTriggerTransport;
use serde_json::Value;
use std::sync::Mutex;

/// One observed delivery: the trigger token the client forwarded and the
/// exact orchestrator payload it built.
#[derive(Clone)]
pub struct CiTriggerDelivery {
    pub trigger_token: String,
    pub payload: Value,
}

/// Deterministic, in-process stand-in for the CI orchestrator's transport.
///
/// The production client is pinned to the fixed loopback orchestrator
/// endpoint (`CiTriggerClient::new` rejects any other URL as an SSRF
/// bound), so delegation is observed by injecting this transport instead
/// of binding the pinned port — a port a live deployment may already own,
/// which previously turned the assertion into a silent skip. There is no
/// skip path here: every delivery is captured and every scripted outcome
/// is served, unconditionally.
///
/// Outcomes are served in script order; once only one remains it repeats,
/// so a one-entry script behaves as a fixed answer.
pub struct ScriptedCiTriggerTransport {
    deliveries: Mutex<Vec<CiTriggerDelivery>>,
    script: Mutex<Vec<Result<Option<String>, String>>>,
}

impl ScriptedCiTriggerTransport {
    pub fn new(script: Vec<Result<Option<String>, String>>) -> Self {
        Self {
            deliveries: Mutex::new(Vec::new()),
            script: Mutex::new(script),
        }
    }

    /// Snapshot of every delivery observed so far.
    pub fn deliveries(&self) -> Vec<CiTriggerDelivery> {
        self.deliveries.lock().unwrap().clone()
    }
}

#[async_trait]
impl CiTriggerTransport for ScriptedCiTriggerTransport {
    async fn send(&self, trigger_token: &str, payload: Value) -> Result<Option<String>, String> {
        let mut deliveries = self.deliveries.lock().unwrap();
        deliveries.push(CiTriggerDelivery {
            trigger_token: trigger_token.to_string(),
            payload,
        });
        let mut script = self.script.lock().unwrap();
        if script.len() > 1 {
            script.remove(0)
        } else {
            match script.first() {
                Some(outcome) => outcome.clone(),
                None => Err("ci trigger script exhausted".to_string()),
            }
        }
    }
}
