use super::*;
use filmcraft_project::{AutomationMode, ItemId};
use filmcraft_time::Tick;
use serde_json::json;
const KIND: &str = "native_project_sequence_view_proof";
const CMD: &str = "agent.selectProjectSequence";
const LIMIT: usize = 4 * 1024 * 1024;
fn editors() -> (EditorState, EditorState) {
    let mut before = EditorState::default();
    before.active_sequence = Some(ItemId(21));
    before.source_playhead = Tick(9007199254740993);
    before.playheads.insert(ItemId(21), Tick(i64::MAX - 1024));
    before.playheads.insert(ItemId(22), Tick(i64::MIN + 1024));
    before.default_audio_transition = "constant_power".into();
    before.default_video_transition = "cross_dissolve".into();
    let mut after = before.clone();
    after.clip_mixer_modes.insert(7, AutomationMode::Touch);
    (before, after)
}
fn fixture() -> Value {
    let (before, after) = editors();
    serde_json::to_value(encode(&before, &after, &"a".repeat(64), KIND, LIMIT, CMD).unwrap()).unwrap()
}
fn rehash(value: &mut Value, change: impl FnOnce(&mut Value)) {
    let mut inner: Value = serde_json::from_str(value["data"].as_str().unwrap()).unwrap();
    change(&mut inner);
    let data = serde_json::to_string(&inner).unwrap();
    value["bytes"] = json!(data.len());
    value["sha256"] = json!(format!("{:x}", Sha256::digest(data.as_bytes())));
    value["data"] = json!(data);
}
#[test]
fn shared_editor_proof_keeps_original_opaque_byte_format_and_exact_native_positive_negative_i64_clocks() {
    let (before, after) = editors();
    let clipboard = "a".repeat(64);
    let proof = encode(&before, &after, &clipboard, KIND, LIMIT, CMD).unwrap();
    let expected_data = format!(
        "{{\"version\":1,\"beforeEditor\":{},\"afterEditor\":{},\"clipboardSha256\":{}}}",
        serde_json::to_string(&serde_json::to_value(&before).unwrap()).unwrap(),
        serde_json::to_string(&serde_json::to_value(&after).unwrap()).unwrap(),
        serde_json::to_string(&clipboard).unwrap()
    );
    assert_eq!(proof.data, expected_data);
    assert!(proof.data.contains("9007199254740993"));
    assert!(proof.data.contains(&(i64::MAX - 1024).to_string()));
    assert!(proof.data.contains(&(i64::MIN + 1024).to_string()));
    let json = serde_json::to_value(&proof).unwrap();
    let decoded = decode(&json, KIND, LIMIT, CMD).unwrap();
    assert_eq!(decoded.before.source_playhead, before.source_playhead);
    assert_eq!(decoded.before.playheads, before.playheads);
    require_after(&decoded.after_editor, &after, CMD).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&serde_json::to_string(&json).unwrap()).unwrap(), json);
}
#[test]
fn shared_editor_proof_rejects_changed_outer_identity_versions_hashes_lengths_and_hidden_fields() {
    for mutation in 0..8 {
        let mut bad = fixture();
        match mutation {
            0 => bad["kind"] = json!("foreign_native_proof"),
            1 => bad["version"] = json!(2),
            2 => bad["sha256"] = json!("f".repeat(64)),
            3 => bad["bytes"] = json!(0),
            4 => bad["bytes"] = json!(bad["bytes"].as_u64().unwrap() + 1),
            5 => bad["extra"] = json!(true),
            6 => bad["data"] = json!("{"),
            _ => bad["bytes"] = json!(1.5),
        }
        assert!(decode(&bad, KIND, LIMIT, CMD).is_err(), "accepted mutation {mutation}");
    }
    assert!(decode(&fixture(), KIND, 1, CMD).is_err());
}
#[test]
fn shared_editor_proof_refuses_rehashed_unknown_dropped_fractional_or_untyped_editor_and_inner_fields() {
    for mutation in 0..8 {
        let mut bad = fixture();
        rehash(&mut bad, |inner| match mutation {
            0 => inner["beforeEditor"]["unknown_native_field"] = json!(true),
            1 => inner["afterEditor"]["unknown_native_field"] = json!(true),
            2 => inner["beforeEditor"]["source_playhead"] = json!(1.5),
            3 => inner["afterEditor"]["clip_mixer_modes"] = json!({"7":"not_a_mode"}),
            4 => inner["version"] = json!(2),
            5 => inner["clipboardSha256"] = json!("invalid"),
            6 => inner["extra"] = json!(true),
            _ => {
                inner["beforeEditor"].as_object_mut().unwrap().remove("snapping");
            }
        });
        assert!(decode(&bad, KIND, LIMIT, CMD).is_err(), "accepted mutation {mutation}");
    }
}
#[test]
fn shared_editor_proof_binds_clipboard_by_hash_without_exporting_or_restoring_private_clipboard_content() {
    let (mut before, mut after) = editors();
    let marker =
        serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"PRIVATE_CLIPBOARD_MARKER","comment":"","color":"Iris","kind":"Comment"})).unwrap();
    before.clipboard_markers.push(marker);
    after.clipboard_markers = before.clipboard_markers.clone();
    let clipboard = crate::agent_bridge::project_clear::clipboard_hash(&before).unwrap();
    let proof = encode(&before, &after, &clipboard, KIND, LIMIT, CMD).unwrap();
    assert!(!proof.data.contains("PRIVATE_CLIPBOARD_MARKER"));
    assert!(!proof.data.contains("clipboard_markers"));
    let decoded = decode(&serde_json::to_value(proof).unwrap(), KIND, LIMIT, CMD).unwrap();
    assert_eq!(decoded.clipboard_sha256, clipboard);
    assert!(decoded.before.clipboard_markers.is_empty());
    assert_eq!(before.clipboard_markers.len(), 1);
    assert_eq!(after.clipboard_markers.len(), 1);
    require_after(&decoded.after_editor, &after, CMD).unwrap();
}
#[test]
fn shared_editor_proof_after_check_requires_exact_caller_computed_state_even_when_a_forgery_is_rehashed() {
    let (_, after) = editors();
    for mutation in 0..3 {
        let mut changed = fixture();
        rehash(&mut changed, |inner| match mutation {
            0 => inner["afterEditor"]["snapping"] = json!(!after.snapping),
            1 => inner["afterEditor"]["default_audio_transition"] = json!("constant_gain"),
            _ => inner["afterEditor"]["source_playhead"] = json!(9007199254740992u64),
        });
        let decoded = decode(&changed, KIND, LIMIT, CMD).unwrap();
        assert!(require_after(&decoded.after_editor, &after, CMD).is_err());
    }
    let correct = decode(&fixture(), KIND, LIMIT, CMD).unwrap();
    require_after(&correct.after_editor, &after, CMD).unwrap();
}
#[test]
fn shared_editor_proof_private_byte_limit_counts_utf8_bytes_at_the_original_boundary() {
    let (mut before, after) = editors();
    before.default_video_transition = "Native 未知 Übergang".into();
    let proof = encode(&before, &after, &"a".repeat(64), KIND, LIMIT, CMD).unwrap();
    assert!(proof.data.len() > proof.data.chars().count());
    let length = proof.data.len();
    let exact = encode(&before, &after, &"a".repeat(64), KIND, length, CMD).unwrap();
    assert_eq!(exact.bytes, length);
    assert!(encode(&before, &after, &"a".repeat(64), KIND, length - 1, CMD).is_err());
    let value = serde_json::to_value(exact).unwrap();
    assert!(decode(&value, KIND, length, CMD).is_ok());
    assert!(decode(&value, KIND, length - 1, CMD).is_err());
}
