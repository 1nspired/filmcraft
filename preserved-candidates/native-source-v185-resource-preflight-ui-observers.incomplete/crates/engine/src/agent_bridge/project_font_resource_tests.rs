use super::{
    project_font_replacement_tests::{populated, scope, setup, state},
    project_plane,
    tests::token,
};
use filmcraft_project::{Caption, ClipId};
use filmcraft_time::Tick;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn query(s: &mut crate::Session, n: usize) -> Value {
    let d = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    json!({"token":token(n),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":{"from":{"family":"Noto Serif"},"to":{"family":"JetBrains Mono","style":"Bold"}},"offset":0,"limit":16})
}
fn plan(s: &mut crate::Session, p: Value) -> Value {
    s.execute("agent.planProjectFontResources", p).unwrap()
}
#[test]
fn project_font_resource_plan_covers_all_original_authored_targets_and_exact_native_samples_without_authoring_or_view_changes() {
    let mut s = populated();
    let q = query(&mut s, 88000);
    let before = state(&s);
    let r = plan(&mut s, q.clone());
    assert_eq!(state(&s), before);
    assert_eq!(r["collateralTotal"], 4);
    assert_eq!(r["total"], 4);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["allFramesVerified"], false);
    assert_eq!(r["writePreconditionsSatisfied"], false);
    assert_eq!(r["sourceValidation"], "not-performed");
    assert_eq!(r["fontFileState"], "not-inspected");
    assert_eq!(r["embeddingRights"], "not-established");
    assert_eq!(r["totals"]["removedFontKeyframes"], 4);
    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|v| v["sampling"]["kind"] == "exact_clip_entry").count(), 2);
    assert_eq!(rows.iter().filter(|v| v["sampling"]["kind"] == "source_master_origin").count(), 1);
    let empty = rows.iter().find(|v| v["sampling"]["kind"] == "empty_caption_track_declared_style").unwrap();
    assert!(empty["sampling"]["caption"].is_null());
    assert_eq!(empty["before"]["kind"], "native_caption_declared_font_resolution");
    assert_eq!(empty["before"]["fontState"]["glyphCount"], 0);
    let again = plan(&mut s, q);
    assert_eq!(r, again);
    assert_eq!(state(&s), before);
}
#[test]
fn project_font_resource_plan_streams_one_full_stable_proof_across_pages_and_binds_original_human_after_project() {
    let mut s = populated();
    let mut q = query(&mut s, 88010);
    q["limit"] = json!(1);
    let before = state(&s);
    let first = plan(&mut s, q.clone());
    let mut rows = vec![];
    let mut r = first.clone();
    loop {
        rows.extend(r["rows"].as_array().unwrap().iter().cloned());
        if let Some(next) = r["nextOffset"].as_u64() {
            q["offset"] = json!(next);
            r = plan(&mut s, q.clone());
            assert_eq!(r["fontPlan"], first["fontPlan"]);
        } else {
            break;
        }
    }
    assert_eq!(rows.len(), 4);
    assert_eq!(state(&s), before);
    let expected = first["fontPlan"]["afterProjectSha256"].clone();
    let human = s.execute("file.replaceFonts", q["replacement"].clone()).unwrap();
    assert_eq!(human, first["humanResult"]);
    assert_eq!(format!("{:x}", Sha256::digest(super::project_audit::bytes(&s.project).unwrap())), expected);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(s.project.as_ref(), &serde_json::from_value::<filmcraft_project::Project>(before["project"].clone()).unwrap());
}
#[test]
fn project_font_resource_plan_preserves_legitimate_subframe_clips_and_each_caption_without_seeking_or_rounding() {
    let mut s = populated();
    let first = s.project.sequences().find(|i| i.name == "First").unwrap().id;
    let rate = s.project.sequence(first).unwrap().settings.frame_rate;
    {
        let p = std::sync::Arc::make_mut(&mut s.project);
        p.sequence_mut(first).unwrap().video_tracks[0].items[0].duration = Tick(1);
        let id1 = ClipId(p.alloc_id());
        let id2 = ClipId(p.alloc_id());
        let track = &mut p.sequence_mut(first).unwrap().caption_tracks[0];
        for (id, start, text) in [(id1, 0, "A😀é"), (id2, 10, "שלום second cue")] {
            track.captions.push(Caption {
                id,
                start: rate.tick_of(start),
                duration: Tick(1),
                text: text.into(),
                speaker: None,
                cue_id: None,
                settings: String::new(),
            });
        }
    }
    let q = query(&mut s, 88020);
    let before = state(&s);
    let r = plan(&mut s, q);
    assert_eq!(r["collateralTotal"], 4);
    assert_eq!(r["total"], 5);
    let rows = r["rows"].as_array().unwrap();
    let cue = rows.iter().filter(|v| v["sampling"]["kind"] == "one_exact_caption_canvas").collect::<Vec<_>>();
    assert_eq!(cue.len(), 2);
    assert!(cue.iter().all(|v| v["sampling"]["durationTicks"] == "1"));
    let short = rows.iter().find(|v| v["target"]["sequence"] == first.0 && v["sampling"]["kind"] == "exact_clip_entry").unwrap();
    assert_eq!(short["sampling"]["timelineTicks"], "0");
    assert_eq!(short["before"]["timelineTicks"], "0");
    assert!(!short["before"].as_object().unwrap().contains_key("frame"));
    assert_eq!(state(&s), before);
}
#[test]
fn project_font_resource_plan_refuses_foreign_stale_null_unknown_and_bad_pages_without_editing_or_consuming_history() {
    let mut s = populated();
    let q = query(&mut s, 88030);
    let before = state(&s);
    for i in 0..8 {
        let mut p = q.clone();
        match i {
            0 => p["scope"]["projectId"] = json!("foreign"),
            1 => p["beforeProjectSha256"] = json!("0".repeat(64)),
            2 => p["replacement"]["from"]["style"] = Value::Null,
            3 => p["limit"] = json!(0),
            4 => p["offset"] = json!(100001),
            5 => p["offset"] = json!(5),
            6 => p["fontPlan"] = json!({}),
            _ => p["token"] = json!(token(999999)),
        };
        assert!(s.execute("agent.planProjectFontResources", p).is_err(), "case{i}");
        assert_eq!(state(&s), before);
    }
    s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
    let later = state(&s);
    assert!(s.execute("agent.planProjectFontResources", q).is_err());
    assert_eq!(state(&s), later);
}
#[test]
fn project_font_resource_plan_keeps_empty_project_noop_and_case_sensitive_proposal_binding_honest() {
    let mut empty = setup();
    let q = query(&mut empty, 88040);
    let before = state(&empty);
    let r = plan(&mut empty, q);
    assert_eq!(r["total"], 0);
    assert_eq!(r["changesAuthoredState"], false);
    assert_eq!(r["writePreconditionsSatisfied"], false);
    assert_eq!(state(&empty), before);
    let mut s = populated();
    let mut q = query(&mut s, 88041);
    let a = plan(&mut s, q.clone());
    q["replacement"]["from"]["family"] = json!("noto serif");
    let b = plan(&mut s, q);
    assert_ne!(a["fontPlan"]["proposalSha256"], b["fontPlan"]["proposalSha256"]);
    assert_eq!(a["fontPlan"]["afterProjectSha256"], b["fontPlan"]["afterProjectSha256"]);
    assert_eq!(a["fontPlan"]["beforeResourcesSha256"], b["fontPlan"]["beforeResourcesSha256"]);
}

pub(super) fn editable() -> crate::Session {
    let mut s = setup();
    s.execute("file.newSequence", json!({"name":"Editable","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
    let c = s.execute("graphics.newText", json!({"text":"Owned font target","font":"Noto Serif","fontStyle":"Regular"})).unwrap()["clip"].clone();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s.execute("graphics.upgradeToSourceGraphic", json!({"clip":c})).unwrap();
    s
}
#[test]
fn project_font_resource_plan_distinguishes_complete_editable_scope_from_locks_protection_untracked_and_unchanged_drifted_members() {
    let mut s = editable();
    let q = query(&mut s, 88050);
    let ready = plan(&mut s, q);
    assert_eq!(ready["writePreconditionsSatisfied"], true);
    assert_eq!(ready["writeAuthority"], false);
    let sid = s.state.active_sequence.unwrap();
    for mode in 0..4 {
        let mut s = editable();
        let sid = s.state.active_sequence.unwrap();
        let p = std::sync::Arc::make_mut(&mut s.project);
        let clip = &mut p.sequence_mut(sid).unwrap().video_tracks[0].items[0];
        match mode {
            0 => p.sequence_mut(sid).unwrap().video_tracks[0].locked = true,
            1 => clip.identity.as_mut().unwrap().protection = Some("Preserve human target".into()),
            2 => {
                p.item_mut(sid).unwrap().metadata.remove(filmcraft_project::clip_identity::EFFECT_TRACKING_KEY);
            }
            _ => {
                let e = clip.effects.iter_mut().find(|e| e.effect == filmcraft_project::graphic::TEXT_LAYER).unwrap();
                e.params.get_mut("font").unwrap().value = filmcraft_project::ParamValue::Text("Inter".into());
            }
        }
        let q = query(&mut s, 88051 + mode);
        let before = state(&s);
        let refused = plan(&mut s, q);
        assert_eq!(refused["writePreconditionsSatisfied"], false, "mode{mode}");
        assert_eq!(state(&s), before);
        assert!(refused["rows"].as_array().unwrap().iter().any(|v| v["eligibility"]["eligible"] == false));
    }
    let _ = sid;
}
struct Face {
    family: String,
}
impl filmcraft_text::fonts::FontSource for Face {
    fn faces(&self) -> Vec<filmcraft_text::fonts::FaceInfo> {
        vec![filmcraft_text::fonts::FaceInfo {
            family: self.family.clone(),
            style: "Bold".into(),
            weight: 700,
            italic: false,
            index: 0,
            data: filmcraft_text::fonts::FaceData::Static(filmcraft_text::fonts::INTER_BOLD),
            origin: "generated-test",
        }]
    }
}
#[test]
fn project_font_resource_plan_binds_relevant_native_resolution_changes_without_invalidating_unrelated_registry_growth() {
    let mut s = editable();
    let mut q = query(&mut s, 88060);
    let family = format!("Poema global late {}", std::process::id());
    q["replacement"]["to"]["family"] = json!(family);
    let before = state(&s);
    let a = plan(&mut s, q.clone());
    assert_eq!(filmcraft_text::fonts::add_source(&Face { family: family.clone() }), 1);
    let b = plan(&mut s, q.clone());
    assert_eq!(a["fontPlan"]["beforeProjectSha256"], b["fontPlan"]["beforeProjectSha256"]);
    assert_eq!(a["fontPlan"]["afterProjectSha256"], b["fontPlan"]["afterProjectSha256"]);
    assert_eq!(a["fontPlan"]["collateralSha256"], b["fontPlan"]["collateralSha256"]);
    assert_ne!(a["fontPlan"]["afterResourcesSha256"], b["fontPlan"]["afterResourcesSha256"]);
    filmcraft_text::fonts::add_source(&Face { family: format!("Poema global unrelated {}", std::process::id()) });
    let c = plan(&mut s, q);
    assert_eq!(b["fontPlan"], c["fontPlan"]);
    assert_eq!(state(&s), before);
}
