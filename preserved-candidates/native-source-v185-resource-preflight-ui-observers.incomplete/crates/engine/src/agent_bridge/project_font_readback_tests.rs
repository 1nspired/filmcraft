use super::{
    project_font_replacement_tests::{scope, state},
    project_font_write_tests::{batch, fixture, read},
    tests::token,
};
use crate::Session;
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use serde_json::{Value, json};
use std::sync::Arc;
fn query(s: &mut Session, n: usize, commit: usize, side: &str) -> Value {
    read(s, n);
    json!({"token":token(n),"scope":scope(),"commit":token(commit),"side":side,"offset":0,"limit":1})
}
fn observe(s: &mut Session, q: Value) -> Value {
    s.execute("agent.readProjectFontResources", q).unwrap()
}
#[test]
fn current_project_font_readback_streams_complete_original_targets_after_apply_undo_and_redo_without_changing_project_or_editor() {
    for empty in [false, true] {
        let mut s = fixture();
        if empty {
            let sid = s.state.active_sequence.unwrap();
            let seq = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
            seq.caption_tracks[0].captions.clear();
            seq.video_tracks[0].items[0].start = filmcraft_time::Tick(1);
        }
        let p = batch(&mut s, 90000);
        let r = s.execute("agent.apply", p.clone()).unwrap();
        let q = query(&mut s, 90001, 90000, "after");
        let before = state(&s);
        let first = observe(&mut s, q.clone());
        assert_eq!(state(&s), before);
        assert_eq!(first["resourcesSha256"], r["results"][0]["fontResources"]["afterResourcesSha256"]);
        assert_eq!(first["projectSha256"], r["results"][0]["afterSha256"]);
        assert_eq!(first["matchesHistoricalResources"], true);
        assert_eq!(first["allFramesVerified"], false);
        assert_eq!(first["writeAuthority"], false);
        assert_eq!(first["historyOwnership"], "not-established");
        let mut page = first.clone();
        let mut all = vec![];
        loop {
            all.extend(page["rows"].as_array().unwrap().iter().cloned());
            let Some(offset) = page["nextOffset"].as_u64() else { break };
            let mut next = q.clone();
            next["offset"] = json!(offset);
            page = observe(&mut s, next);
            for key in ["projectSha256", "resourcesSha256", "samplingSha256", "eligibilitySha256", "collateralSha256"] {
                assert_eq!(page[key], first[key]);
            }
        }
        assert_eq!(all.len() as u64, first["total"].as_u64().unwrap());
        assert_eq!(all.len(), 4);
        assert!(all.iter().any(|x| x["sampling"]["kind"] == "source_master_origin"));
        if empty {
            assert!(all.iter().any(|x| x["sampling"]["kind"] == "empty_caption_track_declared_style"));
            assert!(all.iter().any(|x| x["sampling"]["timelineTicks"] == "1"));
        } else {
            assert!(all.iter().any(|x| x["sampling"]["kind"] == "one_exact_caption_canvas"));
        }
        assert_eq!(state(&s), before);
        read(&mut s, 90002);
        s.execute("agent.undo", json!({"token":token(90002),"commit":token(90000)})).unwrap();
        let q = query(&mut s, 90003, 90000, "before");
        let undone = state(&s);
        let before_read = observe(&mut s, q);
        assert_eq!(before_read["resourcesSha256"], r["results"][0]["fontResources"]["beforeResourcesSha256"]);
        assert_eq!(before_read["projectSha256"], r["results"][0]["beforeSha256"]);
        assert_eq!(state(&s), undone);
        read(&mut s, 90004);
        s.execute("agent.redo", json!({"token":token(90004),"commit":token(90000),"undo":token(90002)})).unwrap();
        let q = query(&mut s, 90005, 90000, "after");
        let redone = state(&s);
        let after_read = observe(&mut s, q);
        assert_eq!(after_read["resourcesSha256"], first["resourcesSha256"]);
        assert_eq!(state(&s), redone);
    }
}
#[test]
fn current_project_font_readback_refuses_wrong_side_foreign_stale_unknown_and_raw_targets_without_any_changes() {
    let mut s = fixture();
    let p = batch(&mut s, 90010);
    s.execute("agent.apply", p).unwrap();
    let q = query(&mut s, 90011, 90010, "after");
    let before = state(&s);
    for mode in 0..10 {
        let mut bad = q.clone();
        match mode {
            0 => bad["scope"]["projectId"] = json!("foreign"),
            1 => bad["commit"] = json!(token(999999)),
            2 => bad["commit"] = bad["token"].clone(),
            3 => bad["side"] = json!("before"),
            4 => bad["side"] = json!("current"),
            5 => bad["offset"] = json!(99999),
            6 => bad["limit"] = json!(17),
            7 => bad["targets"] = json!([]),
            8 => bad["resourceHash"] = json!("0".repeat(64)),
            _ => bad["side"] = Value::Null,
        };
        assert!(s.execute("agent.readProjectFontResources", bad).is_err(), "case{mode}");
        assert_eq!(state(&s), before);
    }
    s.set_playhead(filmcraft_time::Tick(1));
    let changed = state(&s);
    assert!(s.execute("agent.readProjectFontResources", q).is_err());
    assert_eq!(state(&s), changed);
}
struct Face {
    family: String,
}
impl FontSource for Face {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Bold".into(),
            weight: 700,
            italic: false,
            index: 0,
            data: FaceData::Static(fonts::INTER_BOLD),
            origin: "generated-test",
        }]
    }
}
#[test]
fn current_project_font_readback_refreshes_relevant_fonts_and_never_renews_historical_resource_grants_or_blocks_owned_history() {
    let mut s = fixture();
    let mut p = batch(&mut s, 90020);
    let family = format!("Current global font {}", std::process::id());
    p["operations"][0]["params"]["replacement"]["to"]["family"] = json!(family);
    let d = read(&mut s, 90021);
    let planned=s.execute("agent.planProjectFontResources",json!({"token":token(90021),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":p["operations"][0]["params"]["replacement"],"offset":0,"limit":1})).unwrap();
    p["token"] = json!(token(90021));
    p["operations"][0]["params"]["fontPlan"] = planned["fontPlan"].clone();
    let receipt = s.execute("agent.apply", p).unwrap();
    let q = query(&mut s, 90022, 90021, "after");
    let a = observe(&mut s, q.clone());
    let state_before = state(&s);
    assert_eq!(fonts::add_source(&Face { family: family.clone() }), 1);
    let b = observe(&mut s, q.clone());
    assert_ne!(a["resourcesSha256"], b["resourcesSha256"]);
    assert_eq!(a["projectSha256"], b["projectSha256"]);
    assert_eq!(a["matchesHistoricalResources"], true);
    assert_eq!(b["matchesHistoricalResources"], false);
    assert_eq!(b["historicalResourcesSha256"], receipt["results"][0]["fontResources"]["afterResourcesSha256"]);
    fonts::add_source(&Face { family: format!("Unrelated readback {}", std::process::id()) });
    let c = observe(&mut s, q);
    assert_eq!(b, c);
    assert_eq!(state(&s), state_before);
    read(&mut s, 90023);
    s.execute("agent.undo", json!({"token":token(90023),"commit":token(90021)})).unwrap();
    read(&mut s, 90024);
    s.execute("agent.redo", json!({"token":token(90024),"commit":token(90021),"undo":token(90023)})).unwrap();
    let q = query(&mut s, 90025, 90021, "after");
    let d = observe(&mut s, q);
    assert_eq!(d["resourcesSha256"], b["resourcesSha256"]);
    assert_eq!(d["matchesHistoricalResources"], false);
}
#[test]
fn current_project_font_readback_requires_retained_exact_graph_and_current_whole_project_instead_of_a_receipt_alone() {
    for mode in 0..4 {
        let mut s = fixture();
        let p = batch(&mut s, 90030 + mode);
        s.execute("agent.apply", p).unwrap();
        let mut q = query(&mut s, 90040 + mode, 90030 + mode, "after");
        match mode {
            0 => {
                Arc::make_mut(&mut s.project).name = "Later human project".into();
                q = query(&mut s, 90080 + mode, 90030 + mode, "after");
            }
            1 => s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(90030 + mode)).unwrap().after = None,
            2 => {
                s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(90030 + mode)).unwrap().committed.as_mut().unwrap().1["results"][0]["fontResources"]
                    ["samplingSha256"] = json!("0".repeat(64))
            }
            _ => s.path = Some("/generated/foreign-project.fcproj".into()),
        };
        let before = state(&s);
        assert!(s.execute("agent.readProjectFontResources", q).is_err(), "case{mode}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn current_project_font_readback_survives_native_cold_checkpoint_and_owned_undo_redo_without_saved_recipe_execution() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = crate::temp_dir().join(format!("poema-font-readback-cold-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&directory).unwrap();
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _scratch = Scratch(directory.clone());
    let mut s = fixture();
    s.path = Some(directory.join("generated.fcproj").to_string_lossy().into_owned());
    let p = batch(&mut s, 90050);
    let r = s.execute("agent.apply", p).unwrap();
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
    const C: &str = "73fa852d-7a2d-4dc5-a454-e0b0291dd58c";
    read(&mut s, 90051);
    let saved =
        s.execute("agent.saveHistoryCheckpoint", json!({"token":token(90051),"scope":scope(),"workspace":scope()["workspaceId"],"checkpoint":C})).unwrap();
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":s.path})).unwrap();
    read(&mut fresh, 90052);
    fresh
        .execute(
            "agent.restoreHistoryCheckpoint",
            json!({"token":token(90052),"scope":scope(),"workspace":scope()["workspaceId"],"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}),
        )
        .unwrap();
    let q = query(&mut fresh, 90053, 90050, "after");
    let state_before = state(&fresh);
    let a = observe(&mut fresh, q);
    assert_eq!(a["resourcesSha256"], r["results"][0]["fontResources"]["afterResourcesSha256"]);
    assert_eq!(state(&fresh), state_before);
    read(&mut fresh, 90054);
    fresh.execute("agent.undo", json!({"token":token(90054),"commit":token(90050)})).unwrap();
    let q = query(&mut fresh, 90055, 90050, "before");
    assert_eq!(observe(&mut fresh, q)["resourcesSha256"], r["results"][0]["fontResources"]["beforeResourcesSha256"]);
    read(&mut fresh, 90056);
    fresh.execute("agent.redo", json!({"token":token(90056),"commit":token(90050),"undo":token(90054)})).unwrap();
    let q = query(&mut fresh, 90057, 90050, "after");
    assert_eq!(observe(&mut fresh, q)["resourcesSha256"], a["resourcesSha256"]);
}

#[test]
fn current_project_font_readback_does_not_treat_a_no_change_replacement_plan_as_current_resource_verification() {
    let mut s = fixture();
    let p = batch(&mut s, 90060);
    s.execute("agent.apply", p.clone()).unwrap();
    let d = read(&mut s, 90061);
    let noop=s.execute("agent.planProjectFontResources",json!({"token":token(90061),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":p["operations"][0]["params"]["replacement"],"offset":0,"limit":1})).unwrap();
    assert_eq!(noop["total"], 0);
    assert_eq!(noop["changesAuthoredState"], false);
    let before = state(&s);
    let current = observe(&mut s, json!({"token":token(90061),"scope":scope(),"commit":token(90060),"side":"after","offset":0,"limit":1}));
    assert_eq!(current["total"], 4);
    assert_ne!(current["resourcesSha256"], noop["fontPlan"]["beforeResourcesSha256"]);
    assert_eq!(current["matchesHistoricalResources"], true);
    assert_eq!(state(&s), before);
}
