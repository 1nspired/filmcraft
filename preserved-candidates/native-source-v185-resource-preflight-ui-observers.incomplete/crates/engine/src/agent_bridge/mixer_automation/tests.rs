use super::super::tests::{read as snapshot, token};
use super::*;
use crate::Session;
use filmcraft_project::{Interpolation, Keyframe};
use filmcraft_time::Tick;
use std::sync::Arc;
fn fixture() -> (Session, u64, u64) {
    let mut s = super::super::tests::setup();
    s.execute("poema.trackSequence", json!({})).unwrap();
    let anchor = s.state.active_sequence.unwrap().0;
    s.execute(
        "project.associateEditorialOwner",
        json!({"scope":{"projectId":"p_tracks","anchorSequence":anchor},"workspaceId":"00000000-0000-4000-8000-000000000001"}),
    )
    .unwrap();
    let audio = s.active_sequence().unwrap().audio_tracks[0].id.0;
    let bus = s.execute("mixer.addSubmix", json!({"name":"Authored lane bus"})).unwrap()["id"].as_u64().unwrap();
    s.execute("mixer.addSend", json!({"strip":audio,"target":bus})).unwrap();
    for strip in [json!(audio), json!(bus), json!("Mix")] {
        s.execute("mixer.addInsert", json!({"strip":strip,"effect":"amplify"})).unwrap();
    }
    (s, audio, bus)
}
fn params(s: &Session, n: usize, strip: Value, lane: Value, offset: usize, limit: usize) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_tracks","anchorSequence":s.state.active_sequence.unwrap().0},"strip":strip,"lane":lane,"offset":offset,"limit":limit})
}
fn page(s: &mut Session, n: usize, strip: Value, lane: Value, offset: usize, limit: usize) -> Value {
    let p = params(s, n, strip, lane, offset, limit);
    s.execute(COMMAND, p).unwrap()
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
fn catalog(s: &mut Session, n: usize, strip: Value) -> Vec<Value> {
    let mut offset = 0;
    let mut rows = Vec::new();
    loop {
        let p = page(s, n, strip.clone(), Value::Null, offset, 4);
        rows.extend(p["lanes"].as_array().unwrap().clone());
        match p["nextOffset"].as_u64() {
            Some(v) => offset = v as usize,
            None => {
                assert_eq!(rows.len(), p["total"].as_u64().unwrap() as usize);
                break;
            }
        }
    }
    rows
}
fn seed_keys(s: &mut Session, id: u64, lane: &str, count: usize) {
    let sid = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project)
        .sequence_mut(sid)
        .unwrap()
        .with_strip_mut(TrackId(id), |tr| {
            let p = tr.lane_mut(lane).unwrap();
            p.keyframes = (0..count)
                .map(|i| {
                    let mut k = Keyframe::new(Tick(i as i64 * 17), ParamValue::Float(-(i as f64 % 80.0)));
                    k.interp = if i % 2 == 0 { Interpolation::AutoBezier } else { Interpolation::ContinuousBezier };
                    k.in_influence = 0.19;
                    k.out_influence = 0.73;
                    k
                })
                .collect();
        })
        .unwrap();
}
#[test]
fn mixer_authored_catalog_keeps_audio_submix_master_defaults_and_all_native_lane_kinds_readonly() {
    let (mut s, audio, bus) = fixture();
    let doc = snapshot(&mut s, 92000);
    assert_eq!(doc["capabilities"]["mixerAutomationRead"], true);
    let before = state(&s);
    for strip in [json!(audio), json!(bus), Value::Null] {
        let rows = catalog(&mut s, 92000, strip.clone());
        assert!(rows.iter().any(|r| r["key"] == "volume" && r["nativeControlAvailable"] == true));
        assert!(rows.iter().any(|r| r["key"] == "fx.0.gain" && r["storage"] == "insert_parameter" && r["definition"]["id"] == "gain"));
        if strip.is_null() {
            assert!(!rows.iter().any(|r| r["key"] == "pan" || r["key"] == "mute"));
        } else {
            assert!(rows.iter().any(|r| r["key"] == "pan51.center" && r["laneInfo"]["unit"] == "%"));
        }
        if strip == json!(audio) {
            assert!(rows.iter().any(|r| r["key"] == "send.0.level" && r["nativeControlAvailable"] == true));
        }
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r["position"], i + 1);
            assert_eq!(r["keyframes"], 0);
            assert_eq!(r["humanWrittenKeyframeEncoding"], "Float");
        }
    }
    assert_eq!(state(&s), before);
}
#[test]
fn mixer_authored_key_pages_stitch_complete_native_times_values_interpolation_and_influences() {
    let (mut s, audio, _) = fixture();
    seed_keys(&mut s, audio, "volume", 150);
    snapshot(&mut s, 92010);
    let before = state(&s);
    let rows = catalog(&mut s, 92010, json!(audio));
    let lane = rows.iter().position(|r| r["key"] == "volume").unwrap();
    let expected = serde_json::to_value(s.active_sequence().unwrap().mix_track(TrackId(audio)).unwrap().lane_keyframes("volume")).unwrap();
    let mut keys = Vec::new();
    let mut offset = 0;
    loop {
        let r = page(&mut s, 92010, json!(audio), json!(lane), offset, 17);
        assert!(r["lanes"].as_array().unwrap().is_empty());
        assert_eq!(r["descriptor"], rows[lane]);
        assert_eq!(r["timeDomain"], "native_sequence_ticks");
        keys.extend(r["keyframes"].as_array().unwrap().clone());
        match r["nextOffset"].as_u64() {
            Some(v) => offset = v as usize,
            None => break,
        }
    }
    assert_eq!(json!(keys), expected);
    assert_eq!(state(&s), before);
}
#[test]
fn mixer_authored_large_lane_is_paged_without_the_old_complete_insert_page_limit() {
    let (mut s, audio, _) = fixture();
    seed_keys(&mut s, audio, "fx.0.gain", 5000);
    snapshot(&mut s, 92020);
    let rows = catalog(&mut s, 92020, json!(audio));
    let lane = rows.iter().position(|r| r["key"] == "fx.0.gain").unwrap();
    assert_eq!(rows[lane]["keyframes"], 5000);
    let first = page(&mut s, 92020, json!(audio), json!(lane), 0, 64);
    assert_eq!(first["total"], 5000);
    assert_eq!(first["nextOffset"], 64);
    assert_eq!(first["keyframes"].as_array().unwrap().len(), 64);
    let last = page(&mut s, 92020, json!(audio), json!(lane), 4992, 64);
    assert_eq!(last["nextOffset"], Value::Null);
    assert_eq!(last["keyframes"].as_array().unwrap().len(), 8);
    assert_eq!(last["keyframes"][7]["time"], 4999 * 17);
}
#[test]
fn mixer_authored_orphans_aliases_shadowed_fx_storage_and_missing_definition_defaults_stay_distinct() {
    let (mut s, audio, _) = fixture();
    let sid = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
    let tr = q.mix_track_mut(TrackId(audio)).unwrap();
    for (key, v) in [("fx.0.gain", 99.0), ("send.00.level", -7.0), ("send.99.level", -9.0), ("unknown.saved", 3.0)] {
        tr.mixer.lanes.insert(key.into(), Param::new(ParamValue::Float(v)));
    }
    tr.effects[0].params.remove("gain");
    snapshot(&mut s, 92030);
    let before = state(&s);
    let rows = catalog(&mut s, 92030, json!(audio));
    let matching: Vec<_> = rows.iter().filter(|r| r["key"] == "fx.0.gain").collect();
    assert_eq!(matching.len(), 2);
    assert!(matching.iter().any(|r| r["storage"] == "orphan_strip_lane" && r["base"]["Float"].as_f64() == Some(99.0) && r["nativeControlAvailable"] == false));
    assert!(matching.iter().any(|r| r["storage"] == "insert_parameter"
        && r["origin"] == "definition_default"
        && r["base"]["Float"].as_f64() == Some(0.0)
        && r["nativeControlAvailable"] == false));
    assert!(rows.iter().any(|r| r["key"] == "send.00.level" && r["storage"] == "strip_lane" && r["nativeControlAvailable"] == true));
    assert!(rows.iter().any(|r| r["key"] == "send.99.level" && r["nativeControlAvailable"] == false));
    assert_eq!(state(&s), before);
    assert!(!s.active_sequence().unwrap().mix_track(TrackId(audio)).unwrap().effects[0].params.contains_key("gain"));
}
#[test]
fn mixer_authored_bad_pages_foreign_strips_and_implicit_master_requests_preserve_human_redo() {
    let (mut s, audio, _) = fixture();
    s.execute("mixer.setStrip", json!({"strip":audio,"pan":-30})).unwrap();
    s.undo().unwrap();
    snapshot(&mut s, 92040);
    assert!(!s.history.redo.is_empty());
    let base = params(&s, 92040, json!(audio), Value::Null, 0, 4);
    let mut bads = Vec::new();
    for (key, value) in [
        ("strip", json!(0)),
        ("strip", json!(u64::MAX)),
        ("strip", json!("Mix")),
        ("lane", json!(4096)),
        ("lane", json!(-1)),
        ("offset", json!(65537)),
        ("limit", json!(0)),
        ("limit", json!(17)),
        ("projection", json!("true")),
        ("extra", json!(true)),
    ] {
        let mut p = base.clone();
        p[key] = value;
        bads.push(p);
    }
    for key in ["strip", "lane"] {
        let mut p = base.clone();
        p.as_object_mut().unwrap().remove(key);
        bads.push(p);
    }
    let mut p = base.clone();
    p["scope"]["projectId"] = json!("foreign");
    bads.push(p);
    let mut p = base.clone();
    p["strip"] = json!(s.active_sequence().unwrap().video_tracks[0].id.0);
    bads.push(p);
    for p in bads {
        let before = state(&s);
        assert!(s.execute(COMMAND, p.clone()).is_err(), "accepted {p}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn mixer_authored_parameter_validation_refuses_hostile_hidden_keys_types_and_inventory_bounds() {
    for kind in 0..8 {
        let (mut s, audio, _) = fixture();
        let sid = s.state.active_sequence.unwrap();
        let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
        let tr = q.mix_track_mut(TrackId(audio)).unwrap();
        let p = tr.lane_mut("volume").unwrap();
        p.keyframes = vec![Keyframe::new(Tick(10), ParamValue::Float(1.0)), Keyframe::new(Tick(20), ParamValue::Float(2.0))];
        match kind {
            0 => p.keyframes[1].time = Tick(10),
            1 => p.keyframes[1].time = Tick(SAFE + 1),
            2 => p.keyframes[0].value = ParamValue::Bool(true),
            3 => p.keyframes[0].out_influence = f64::NAN,
            4 => p.value = ParamValue::Float(f64::INFINITY),
            5 => {
                tr.mixer.lanes.insert("".into(), Param::new(ParamValue::Float(0.0)));
            }
            6 => p.keyframes = vec![Keyframe::new(Tick(0), ParamValue::Float(0.0)); MAX_KEYS + 1],
            _ => {
                for i in 0..MAX_LANES + 1 {
                    tr.mixer.lanes.insert(format!("orphan.{i}"), Param::new(ParamValue::Float(0.0)));
                }
            }
        }
        let q = s.active_sequence().unwrap();
        let tr = q.strip(TrackId(audio)).unwrap();
        assert!(inventory(q, &tr, TrackId(audio)).is_err(), "accepted hostile mode {kind}");
    }
}
#[test]
fn mixer_authored_reads_reject_changed_documents_same_byte_replacement_and_unpaged_mutation() {
    for mode in 0..3 {
        let (mut s, audio, _) = fixture();
        seed_keys(&mut s, audio, "volume", 150);
        snapshot(&mut s, 92050);
        let rows = catalog(&mut s, 92050, json!(audio));
        let lane = rows.iter().position(|r| r["key"] == "volume").unwrap();
        let old = s.project.clone();
        if mode == 0 {
            s.execute("mixer.setStrip", json!({"strip":audio,"pan":40})).unwrap();
        } else if mode == 1 {
            s.project = Arc::new((*s.project).clone());
        } else {
            let sid = s.state.active_sequence.unwrap();
            Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap().mix_track_mut(TrackId(audio)).unwrap().lane_mut("volume").unwrap().keyframes[149]
                .in_influence = 0.91;
        }
        let before = state(&s);
        let p = params(&s, 92050, json!(audio), json!(lane), 64, 32);
        assert!(s.execute(COMMAND, p).is_err());
        assert_eq!(state(&s), before);
        assert!(Arc::strong_count(&old) > 0);
    }
}
#[test]
fn mixer_authored_projection_does_not_grant_source_freshness_edit_or_live_recorder_authority() {
    let (mut s, audio, _) = fixture();
    snapshot(&mut s, 92060);
    let mut p = params(&s, 92060, json!(audio), Value::Null, 0, 4);
    p["projection"] = json!(true);
    let before = state(&s);
    let r = s.execute(COMMAND, p).unwrap();
    assert_eq!(r["requiresSourceValidation"], true);
    assert_eq!(r["referenceScope"], "exact_read_ticket_only");
    assert_eq!(state(&s), before);
    let batch = json!({"token":token(92060),"label":"No raw mixer authority","exact":true,"operations":[{"command":"mixer.setKeyframe","params":{"strip":audio,"lane":"volume","time":0,"value":-6}}]});
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
    s.execute("mixer.recordStart", json!({"time":0})).unwrap();
    let before = state(&s);
    let p = params(&s, 92060, json!(audio), Value::Null, 0, 4);
    assert!(s.execute(COMMAND, p).is_err());
    assert_eq!(state(&s), before);
    s.execute("mixer.recordStop", json!({"time":0})).unwrap();
}
