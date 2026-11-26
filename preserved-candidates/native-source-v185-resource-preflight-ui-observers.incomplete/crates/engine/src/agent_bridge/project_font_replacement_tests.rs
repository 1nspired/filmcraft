use super::{project_audit, project_plane, tests::token};
use crate::Session;
use filmcraft_project::{
    CaptionFormat, CaptionTrack, ItemId, ParamValue, TrackId, graphic,
    graphic_design::{CharStyle, StyleRun},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
pub(super) fn scope() -> Value {
    json!({"projectId":"p_font_replace_plan","workspaceId":"1d745fc6-5775-4a81-b4d1-9432be8ef329"})
}
pub(super) fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"path":s.path})
}
fn digest(s: &Session) -> String {
    format!("{:x}", Sha256::digest(project_audit::bytes(&s.project).unwrap()))
}
pub(super) fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Replacement plan","workspace":scope()})).unwrap();
    s.path = Some("/generated/replacement-plan.fcproj".into());
    s
}
pub(super) fn populated() -> Session {
    let mut s = setup();
    s.execute("file.newSequence", json!({"name":"First","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
    let c = s.execute("graphics.newText", json!({"text":"A😀é more","font":"Noto Serif","fontStyle":"Regular"})).unwrap()["clip"].as_u64().unwrap();
    let source = ItemId(s.execute("graphics.upgradeToSourceGraphic", json!({"clip":c})).unwrap()["item"].as_u64().unwrap());
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let update = |e: &mut filmcraft_project::EffectInstance| {
        e.params.get_mut("font").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Only animated family".into()));
        e.params.get_mut("font_style").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Italic".into()));
        e.layer.get_or_insert_with(Default::default).runs = vec![
            StyleRun { start: 0, end: 1, style: CharStyle { font: Some("Noto Serif".into()), font_style: Some("Regular".into()), ..Default::default() } },
            StyleRun { start: 1, end: 2, style: CharStyle { font_style: Some("Italic".into()), ..Default::default() } },
            StyleRun { start: 2, end: 3, style: CharStyle { font: Some("Inter".into()), ..Default::default() } },
        ];
    };
    let p = Arc::make_mut(&mut s.project);
    update(p.sequence_mut(sid).unwrap().video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == graphic::TEXT_LAYER).unwrap());
    update(&mut p.source_graphics.get_mut(&source).unwrap().layers[0]);
    let id = TrackId(p.alloc_id());
    let mut captions = CaptionTrack::new(id, "Captions".into(), CaptionFormat::Subtitle);
    captions.style.font = "Noto Serif".into();
    captions.locked = true;
    p.sequence_mut(sid).unwrap().caption_tracks.push(captions);
    p.sequence_mut(sid).unwrap().video_tracks[0].locked = true;
    s.execute("file.newSequence", json!({"name":"Second","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
    s.execute("graphics.newText", json!({"text":"Second title","font":"Noto Serif","fontStyle":"Regular"})).unwrap();
    s
}
fn read(s: &mut Session, n: usize) {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
}
fn args(s: &Session, n: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"beforeProjectSha256":digest(s),"replacement":{"from":{"family":"noto serif"},"to":{"family":"JetBrains Mono","style":"Bold"}},"offset":0,"limit":16})
}
fn plan(s: &mut Session, p: Value) -> Value {
    s.execute("agent.planProjectFontReplacement", p).unwrap()
}
#[test]
fn project_font_replacement_plan_predicts_exact_human_project_and_complete_keyframe_run_master_caption_collateral_without_writes() {
    for style in [false, true] {
        let mut s = populated();
        read(&mut s, 86000);
        let before = state(&s);
        let mut p = args(&s, 86000);
        if style {
            p["replacement"]["from"]["style"] = json!("regular");
        }
        let result = plan(&mut s, p.clone());
        assert_eq!(state(&s), before);
        assert_eq!(result["writeAuthority"], false);
        assert_eq!(result["resourceEvidence"], false);
        assert_eq!(result["rendererSampling"], "not-performed");
        assert_eq!(result["fontFileState"], "not-inspected");
        assert_eq!(result["embeddingRights"], "not-established");
        assert_eq!(result["total"], if style { 3 } else { 4 });
        assert_eq!(result["totals"]["textLayers"], 2);
        assert_eq!(result["totals"]["sourceMasterLayers"], 1);
        assert_eq!(result["totals"]["captionTracks"], if style { 0 } else { 1 });
        assert_eq!(result["totals"]["removedFontKeyframes"], 4);
        assert_eq!(result["totals"]["changedSavedRuns"], 2);
        assert!(result["rows"].as_array().unwrap().iter().any(|r| r["locator"]["trackLocked"] == true));
        assert!(result["rows"].as_array().unwrap().iter().any(|r| r["locator"]["kind"] == "source_master"));
        let row = result["rows"].as_array().unwrap().iter().find(|r| r["removedFontKeyframes"] == 2).unwrap();
        assert_eq!(row["parameters"].as_array().unwrap().len(), 2);
        let human = s.execute("file.replaceFonts", p["replacement"].clone()).unwrap();
        assert_eq!(human, result["humanResult"]);
        assert_eq!(human["replaced"], if style { 5 } else { 6 });
        assert_eq!(digest(&s), result["afterProjectSha256"]);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.project.as_ref(), &serde_json::from_value::<filmcraft_project::Project>(before["project"].clone()).unwrap());
        s.execute("edit.redo", json!({})).unwrap();
        assert_eq!(digest(&s), result["afterProjectSha256"]);
    }
}
#[test]
fn project_font_replacement_plan_pages_one_complete_stable_collection_with_full_proposal_binding() {
    let mut s = populated();
    read(&mut s, 86010);
    let before = state(&s);
    let mut p = args(&s, 86010);
    p["limit"] = json!(1);
    let first = plan(&mut s, p.clone());
    let mut rows = vec![];
    let mut page = first.clone();
    loop {
        rows.extend(page["rows"].as_array().unwrap().iter().cloned());
        if let Some(next) = page["nextOffset"].as_u64() {
            p["offset"] = json!(next);
            page = plan(&mut s, p.clone());
            for name in ["proposalSha256", "collateralSha256", "beforeProjectSha256", "afterProjectSha256", "totals", "total"] {
                assert_eq!(page[name], first[name]);
            }
        } else {
            break;
        }
    }
    assert_eq!(rows.len(), 4);
    assert_eq!(format!("{:x}", Sha256::digest(project_audit::value_bytes(&rows).unwrap())), first["collateralSha256"]);
    p["offset"] = json!(0);
    p["replacement"]["to"]["style"] = json!("Italic");
    let changed = plan(&mut s, p.clone());
    assert_ne!(changed["proposalSha256"], first["proposalSha256"]);
    assert_ne!(changed["afterProjectSha256"], first["afterProjectSha256"]);
    assert_eq!(state(&s), before);
    s.execute(project_plane::VALIDATE, json!({"token":token(86010),"scope":scope()})).unwrap();
    p["offset"] = json!(4);
    assert_eq!(plan(&mut s, p)["rows"], json!([]));
}
#[test]
fn project_font_replacement_plan_reports_no_op_and_empty_project_without_inventing_rendered_or_keyframe_only_matches() {
    let mut empty = setup();
    read(&mut empty, 86020);
    let p = args(&empty, 86020);
    let before = state(&empty);
    let out = plan(&mut empty, p);
    assert_eq!(out["total"], 0);
    assert_eq!(out["changesAuthoredState"], false);
    assert_eq!(out["humanResult"]["replaced"], 0);
    assert_eq!(state(&empty), before);
    let mut s = populated();
    read(&mut s, 86021);
    let mut p = args(&s, 86021);
    p["replacement"]["from"]["family"] = json!("Only animated family");
    let out = plan(&mut s, p);
    assert_eq!(out["total"], 0);
    assert_eq!(out["changesAuthoredState"], false);
    assert_eq!(out["humanResult"]["replaced"], 0);
}
#[test]
fn project_font_replacement_plan_refuses_unknown_null_stale_foreign_and_unbounded_queries_without_state_or_history_changes() {
    let mut s = populated();
    read(&mut s, 86030);
    let p = args(&s, 86030);
    let before = state(&s);
    for i in 0..15 {
        let mut q = p.clone();
        match i {
            0 => q["replacement"]["from"]["style"] = Value::Null,
            1 => q["replacement"]["to"]["style"] = Value::Null,
            2 => q["replacement"]["from"]["family"] = json!(""),
            3 => q["replacement"]["to"]["family"] = json!("bad\0name"),
            4 => q["replacement"]["to"]["family"] = json!("x".repeat(16 * 1024 + 1)),
            5 => q["replacement"]["from"]["extra"] = json!(true),
            6 => q["replacement"]["toStyle"] = json!("Bold"),
            7 => q["projection"] = json!(true),
            8 => q["scope"]["projectId"] = json!("foreign"),
            9 => q["beforeProjectSha256"] = json!("0".repeat(64)),
            10 => q["token"] = json!(token(999999)),
            11 => q["limit"] = json!(0),
            12 => q["offset"] = json!(100001),
            13 => q["offset"] = json!(5),
            _ => q["limit"] = json!(17),
        };
        assert!(s.execute("agent.planProjectFontReplacement", q).is_err(), "case{i}");
        assert_eq!(state(&s), before);
    }
    s.execute("file.newBin", json!({"name":"Human change"})).unwrap();
    let later = state(&s);
    assert!(s.execute("agent.planProjectFontReplacement", p).is_err());
    assert_eq!(state(&s), later);
}

#[test]
fn project_font_replacement_plan_advertises_only_project_metadata_capability_and_exact_human_location_names() {
    let mut s = populated();
    let before = state(&s);
    let d = s.execute(project_plane::READ, json!({"token":token(86040),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(d["capabilities"]["projectFontReplacementPlanning"], true);
    let p = args(&s, 86040);
    let r = plan(&mut s, p);
    assert!(r["rows"].as_array().unwrap().iter().any(|v| v["locator"]["sequenceName"] == "First"));
    assert!(r["rows"].as_array().unwrap().iter().any(|v| v["locator"]["sequenceName"] == "Second"));
    assert!(r["rows"].as_array().unwrap().iter().any(|v| v["locator"]["trackName"] == "Captions"));
    assert!(r["rows"].as_array().unwrap().iter().any(|v| v["locator"]["kind"] == "source_master" && v["locator"]["sourceName"].is_string()));
    assert_eq!(state(&s), before);
}
