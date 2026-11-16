//! Entity availability is independent of length and lifecycle completeness.
//! A metadata-only tunnel has no captured entity, even when its compatibility
//! bodyBase64 field is empty. Historical local captures retain their semantics.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const VARIANTS: [&str; 4] = ["request", "response", "originalRequest", "originalResponse"];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BodyCaptureState { Recorded, NotRecorded }

fn legacy_state(origin: Option<&str>) -> BodyCaptureState {
    if origin == Some("android-lan") { BodyCaptureState::NotRecorded } else { BodyCaptureState::Recorded }
}

/// Metadata projections omit bodyBase64, so this must never inspect body bytes.
/// A malformed declaration fails closed instead of fabricating an empty entity.
pub fn state_from_metadata(metadata: &Value, variant: &str) -> BodyCaptureState {
    if !VARIANTS.contains(&variant) { return BodyCaptureState::NotRecorded; }
    match metadata.get("bodyCapture") {
        Some(Value::Object(states)) => match states.get(variant) {
            Some(Value::String(value)) if value == "recorded" => BodyCaptureState::Recorded,
            Some(Value::String(value)) if value == "notRecorded" => BodyCaptureState::NotRecorded,
            None => legacy_state(metadata.pointer("/trafficOrigin/entry").and_then(Value::as_str)),
            _ => BodyCaptureState::NotRecorded,
        },
        None => legacy_state(metadata.pointer("/trafficOrigin/entry").and_then(Value::as_str)),
        _ => BodyCaptureState::NotRecorded,
    }
}

/// Carry legacy tunnel semantics into JSON snapshots, even if provenance is
/// later relabelled by an importer. Explicit recorded states remain authoritative.
pub fn annotate_metadata(metadata: &mut Value) {
    if metadata.pointer("/trafficOrigin/entry").and_then(Value::as_str) != Some("android-lan") { return; }
    let Some(object) = metadata.as_object_mut() else { return; };
    let states = object.entry("bodyCapture").or_insert_with(|| Value::Object(Map::new()));
    let Some(states) = states.as_object_mut() else { return; };
    for variant in VARIANTS { states.entry(variant).or_insert_with(|| Value::String("notRecorded".into())); }
}

impl crate::model::Flow {
    pub fn body_capture_state(&self, variant: &str) -> BodyCaptureState {
        if !VARIANTS.contains(&variant) { return BodyCaptureState::NotRecorded; }
        self.body_capture.get(variant).copied().unwrap_or_else(||
            legacy_state(self.traffic_origin.as_ref().map(|origin| origin.entry.as_str())))
    }

    pub fn ensure_body_recorded(&self, variant: &str) -> Result<()> {
        ensure!(VARIANTS.contains(&variant), "Invalid body variant");
        ensure!(self.body_capture_state(variant) == BodyCaptureState::Recorded,
            "Requested body was not recorded; metadata is not a complete body");
        Ok(())
    }

    pub fn normalize_body_capture(&mut self) {
        if self.traffic_origin.as_ref().is_some_and(|origin| origin.entry == "android-lan") {
            for variant in VARIANTS { self.body_capture.entry(variant.into()).or_insert(BodyCaptureState::NotRecorded); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_tunnel_is_not_an_empty_capture_and_explicit_states_are_per_entity() {
        let mut value = json!({"trafficOrigin":{"entry":"android-lan"},"request":{"bodyBase64":""}});
        for variant in VARIANTS { assert_eq!(state_from_metadata(&value, variant), BodyCaptureState::NotRecorded); }
        value["bodyCapture"] = json!({"request":"recorded"});
        annotate_metadata(&mut value);
        assert_eq!(state_from_metadata(&value,"request"),BodyCaptureState::Recorded);
        assert_eq!(value["bodyCapture"]["response"],"notRecorded");
        value["trafficOrigin"] = Value::Null;
        assert_eq!(state_from_metadata(&value,"response"),BodyCaptureState::NotRecorded);
        assert_eq!(state_from_metadata(&json!({"request":{"bodyBase64":""}}),"request"),BodyCaptureState::Recorded);
    }

    #[test]
    fn malformed_metadata_cannot_claim_body_availability() {
        for body_capture in [json!(null),json!(false),json!("recorded"),json!({"request":"other"}),json!({"request":false})] {
            assert_eq!(state_from_metadata(&json!({"bodyCapture":body_capture}),"request"),BodyCaptureState::NotRecorded);
        }
        assert_eq!(state_from_metadata(&json!({}),"other"),BodyCaptureState::NotRecorded);
    }

    #[test]
    fn flow_round_trip_and_normalization_keep_body_state_independent_of_lifecycle() {
        let mut value=json!({"id":"test","startedAt":0,"durationMs":0,"source":"capture","state":"completed",
            "trafficOrigin":{"entry":"android-lan","clientAddress":null,"process":null,"processStatus":"remote"},
            "request":{"method":"POST","url":"http://example.invalid","headers":[],"bodyBase64":""},
            "response":null,"error":null,"notes":[]});
        let mut flow:crate::model::Flow=serde_json::from_value(value.clone()).unwrap();
        assert!(flow.ensure_body_recorded("request").is_err());
        flow.normalize_body_capture();flow.traffic_origin=None;
        assert!(flow.ensure_body_recorded("response").is_err());
        let serialized=serde_json::to_value(&flow).unwrap();
        assert_eq!(serialized["bodyCapture"]["request"],"notRecorded");
        value["bodyCapture"]=json!({"request":"recorded"});
        let flow:crate::model::Flow=serde_json::from_value(value).unwrap();
        flow.ensure_body_recorded("request").unwrap();
        assert!(flow.ensure_body_recorded("originalRequest").is_err());
        assert!(flow.ensure_body_recorded("invalid").is_err());
        let mut local=serialized;local.as_object_mut().unwrap().remove("bodyCapture");
        let local:crate::model::Flow=serde_json::from_value(local).unwrap();
        local.ensure_body_recorded("request").unwrap();
    }
}
