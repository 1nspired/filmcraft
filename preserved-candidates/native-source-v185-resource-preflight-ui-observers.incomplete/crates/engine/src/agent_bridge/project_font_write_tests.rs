use super::{
    Batch,
    project_font_replacement_tests::{scope, state},
    project_font_resource_tests::editable,
    project_font_write, project_plane,
    tests::token,
};
use crate::Session;
use filmcraft_project::{ParamValue, graphic};
use serde_json::{Value, json};
use std::sync::Arc;
pub(super) fn fixture() -> Session {
    let mut s = editable();
    let first = s.state.active_sequence.unwrap();
    let source = s.project.sequence(first).unwrap().video_tracks[0].items[0].item;
    let clip = s.project.sequence(first).unwrap().video_tracks[0].items[0].id;
    s.execute("graphics.set", json!({"clip":clip.0,"layer":0,"time":0,"props":{"text":"A😀é owned more"}})).unwrap();
    s.execute("graphics.setCharStyle", json!({"clip":clip.0,"layer":0,"start":0,"end":2,"style":{"font":"Noto Serif","font_style":"Regular"}})).unwrap();
    s.execute("captions.add", json!({"text":"Caption words","seconds":3,"durationSeconds":1})).unwrap();
    let rate = s.sequence_rate();
    let p = Arc::make_mut(&mut s.project);
    p.sequence_mut(first).unwrap().caption_tracks[0].style.font = "Noto Serif".into();
    let update = |e: &mut filmcraft_project::EffectInstance| {
        e.params.get_mut("font").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Animated old font".into()));
        e.params.get_mut("font_style").unwrap().put_keyframe(rate.tick_of(10), ParamValue::Text("Italic".into()));
    };
    update(p.sequence_mut(first).unwrap().video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == graphic::TEXT_LAYER).unwrap());
    update(&mut p.source_graphics.get_mut(&source).unwrap().layers[0]);
    let owner = crate::editorial_workspace::association(&s.project).unwrap().unwrap().scope;
    s.execute("file.newSequence", json!({"name":"Other owned view","fps":60,"width":320,"height":180,"video":1,"audio":1})).unwrap();
    let second = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(second).unwrap().metadata.insert(super::sequences::KEY.into(), serde_json::to_string(&owner).unwrap());
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    s.execute("timeline.place", json!({"item":source.0,"track":"V1","time":0,"sourceIn":0,"duration":s.sequence_rate().tick_of(48).0})).unwrap();
    s.execute("sequence.open", json!({"item":first.0})).unwrap();
    s
}
pub(super) fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
pub(super) fn batch(s: &mut Session, n: usize) -> Value {
    let d = read(s, n);
    let replacement = json!({"from":{"family":"Noto Serif"},"to":{"family":"JetBrains Mono","style":"Bold"}});
    let p = s
        .execute(
            "agent.planProjectFontResources",
            json!({"token":token(n),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":replacement,"offset":0,"limit":1}),
        )
        .unwrap();
    assert_eq!(p["writePreconditionsSatisfied"], true);
    json!({"token":token(n),"label":"Replace project fonts","exact":true,"authority":"document_state","operations":[{"command":project_font_write::COMMAND,"params":{"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":replacement,"fontPlan":p["fontPlan"]}}]})
}
fn human(s: &Session) -> Session {
    Session { project: s.project.clone(), state: s.state.clone(), history: s.history.clone(), revision: s.revision, path: s.path.clone(), ..Session::default() }
}
#[test]
fn guarded_project_font_publication_matches_complete_original_human_command_and_retains_one_owned_undo_redo_and_receipt_without_replay() {
    let mut s = fixture();
    let mut h = human(&s);
    let before = s.project.clone();
    let editor = super::project_clear::editor_hash(&s.state).unwrap();
    let count = s.history.undo.len();
    let p = batch(&mut s, 89000);
    let r = s.execute("agent.apply", p.clone()).unwrap();
    h.execute("file.replaceFonts", p["operations"][0]["params"]["replacement"].clone()).unwrap();
    assert_eq!(*s.project, *h.project);
    assert_eq!(super::project_clear::editor_hash(&s.state).unwrap(), editor);
    assert_eq!(s.history.undo.len(), count + 1);
    assert_eq!(r["action"], "project_font_replace");
    assert_eq!(r["authority"], "document_state");
    assert_eq!(r["results"][0]["fontResources"], p["operations"][0]["params"]["fontPlan"]);
    assert_eq!(r["results"][0]["totals"]["removedFontKeyframes"], 6);
    let after = s.project.clone();
    let rev = s.revision;
    assert_eq!(s.execute("agent.receipt", json!({"token":token(89000)})).unwrap()["result"], r);
    assert_eq!(s.execute("agent.apply", p).unwrap(), r);
    assert_eq!(s.revision, rev);
    assert_eq!(s.history.undo.len(), count + 1);
    read(&mut s, 89001);
    s.execute("agent.undo", json!({"token":token(89001),"commit":token(89000)})).unwrap();
    assert_eq!(*s.project, *before);
    read(&mut s, 89002);
    s.execute("agent.redo", json!({"token":token(89002),"commit":token(89000),"undo":token(89001)})).unwrap();
    assert_eq!(*s.project, *after);
    assert_eq!(super::project_clear::editor_hash(&s.state).unwrap(), editor);
}
#[test]
fn guarded_project_font_publication_refuses_forged_tokens_hashes_scope_authority_and_mixed_operations_without_changes() {
    for i in 0..9 {
        let mut s = fixture();
        let mut p = batch(&mut s, 89010 + i);
        let before = state(&s);
        match i {
            0 => p["operations"][0]["params"]["fontPlan"]["readToken"] = json!(token(999999)),
            1 => p["operations"][0]["params"]["fontPlan"]["afterResourcesSha256"] = json!("0".repeat(64)),
            2 => p["operations"][0]["params"]["fontPlan"]["eligibilitySha256"] = json!("0".repeat(64)),
            3 => p["operations"][0]["params"]["replacement"]["from"]["style"] = Value::Null,
            4 => p["operations"][0]["params"]["scope"]["projectId"] = json!("foreign"),
            5 => p["authority"] = json!("media_inputs"),
            6 => {
                let op = p["operations"][0].clone();
                p["operations"].as_array_mut().unwrap().push(op);
            }
            7 => p["operations"][0]["params"]["fontPlan"]["writeAuthority"] = json!(true),
            _ => p["operations"][0]["params"]["beforeProjectSha256"] = json!("0".repeat(64)),
        };
        assert!(s.execute("agent.apply", p).is_err(), "case{i}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn guarded_project_font_publication_rejects_human_view_changed_after_staging_without_overwriting_it() {
    let mut s = fixture();
    let p = batch(&mut s, 89030);
    let second = s.project.sequences().find(|i| i.name == "Other owned view").unwrap().id;
    let before = s.project.clone();
    let count = s.history.undo.len();
    let mut changed_view = None;
    let result = super::operation_control::run(&mut s, "agent.apply", &p, |s, p| {
        let b: Batch = serde_json::from_value(p.clone()).unwrap();
        let stage = super::stage_batch(s, &b)?;
        s.execute("sequence.open", json!({"item":second.0}))?;
        changed_view = Some(super::project_clear::editor_hash(&s.state)?);
        super::publish_batch(s, &b, stage, p, None)
    });
    assert!(result.is_err());
    assert_eq!(*s.project, *before);
    assert_eq!(s.history.undo.len(), count);
    assert_eq!(s.state.active_sequence, Some(second));
    assert_eq!(Some(super::project_clear::editor_hash(&s.state).unwrap()), changed_view);
}

#[test]
fn guarded_project_font_stop_before_queue_and_after_staging_cannot_publish_or_replay() {
    for (index, staged) in [false, true].into_iter().enumerate() {
        let mut s = fixture();
        let n = 89040 + index;
        let p = batch(&mut s, n);
        let before = state(&s);
        let control = s.agent_operation_control();
        let result = if staged {
            super::operation_control::run(&mut s, "agent.apply", &p, |s, p| {
                let b: Batch = serde_json::from_value(p.clone()).unwrap();
                let stage = super::stage_batch(s, &b)?;
                assert_eq!(control.cancel(&json!({"token":token(n)})).unwrap()["cancellationAccepted"], true);
                super::publish_batch(s, &b, stage, p, None)
            })
        } else {
            assert_eq!(control.cancel(&json!({"token":token(n)})).unwrap()["cancellationAccepted"], true);
            s.execute("agent.apply", p.clone())
        };
        assert!(result.is_err());
        assert_eq!(state(&s), before);
        assert!(s.execute("agent.apply", p).is_err());
        assert_eq!(state(&s), before);
    }
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
fn guarded_project_font_late_matching_font_or_scope_changes_refuse_whole_live_publication_while_unrelated_fonts_remain_valid() {
    for mode in 0..4 {
        let mut s = fixture();
        let n = 89050 + mode;
        let mut p = batch(&mut s, n);
        let family = format!("Global publication late {} {mode}", std::process::id());
        if mode < 2 {
            p["operations"][0]["params"]["replacement"]["to"]["family"] = json!(family);
            let d = read(&mut s, n + 20);
            let resources=s.execute("agent.planProjectFontResources",json!({"token":token(n+20),"scope":scope(),"beforeProjectSha256":d["projectState"]["sha256"],"replacement":p["operations"][0]["params"]["replacement"],"offset":0,"limit":1})).unwrap();
            p["token"] = json!(token(n + 20));
            p["operations"][0]["params"]["fontPlan"] = resources["fontPlan"].clone();
        }
        let before = s.project.clone();
        let count = s.history.undo.len();
        let result = super::operation_control::run(&mut s, "agent.apply", &p, |s, p| {
            let b: Batch = serde_json::from_value(p.clone()).unwrap();
            let stage = super::stage_batch(s, &b)?;
            if mode < 2 {
                filmcraft_text::fonts::add_source(&Face {
                    family: if mode == 0 { family.clone() } else { format!("Unrelated publication {}", std::process::id()) },
                });
            } else {
                let sid = s.state.active_sequence.unwrap();
                let q = Arc::make_mut(&mut s.project).sequence_mut(sid).unwrap();
                if mode == 2 {
                    q.video_tracks[0].locked = true;
                } else {
                    q.video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("Later human protection".into());
                }
            }
            super::publish_batch(s, &b, stage, p, None)
        });
        if mode == 1 {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(s.history.undo.len(), count + 1);
        } else {
            assert!(result.is_err());
            assert_eq!(s.history.undo.len(), count);
            if mode == 0 {
                assert_eq!(*s.project, *before);
            } else {
                assert_eq!(s.project.source_graphics, before.source_graphics);
            }
        }
    }
}

#[test]
fn guarded_project_font_cold_checkpoint_restores_owned_undo_redo_and_keeps_resource_proof_historical() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let directory = crate::temp_dir().join(format!("poema-global-font-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
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
    let before = s.project.clone();
    let p = batch(&mut s, 89100);
    let receipt = s.execute("agent.apply", p.clone()).unwrap();
    let after = s.project.clone();
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
    const C: &str = "5d91058d-3345-46da-938d-b2d4b777134c";
    read(&mut s, 89101);
    let saved =
        s.execute("agent.saveHistoryCheckpoint", json!({"token":token(89101),"scope":scope(),"workspace":scope()["workspaceId"],"checkpoint":C})).unwrap();
    let mut fresh = Session::default();
    fresh.execute("file.open", json!({"path":s.path})).unwrap();
    read(&mut fresh, 89102);
    let restored = fresh
        .execute(
            "agent.restoreHistoryCheckpoint",
            json!({"token":token(89102),"scope":scope(),"workspace":scope()["workspaceId"],"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}),
        )
        .unwrap();
    assert_eq!(restored["status"], "committed");
    assert_eq!(*fresh.project, *after);
    assert_eq!(fresh.execute("agent.receipt", json!({"token":token(89100)})).unwrap()["result"], receipt);
    read(&mut fresh, 89103);
    fresh.execute("agent.undo", json!({"token":token(89103),"commit":token(89100)})).unwrap();
    assert_eq!(*fresh.project, *before);
    read(&mut fresh, 89104);
    fresh.execute("agent.redo", json!({"token":token(89104),"commit":token(89100),"undo":token(89103)})).unwrap();
    assert_eq!(*fresh.project, *after);
    let op: super::Operation = serde_json::from_value(p["operations"][0].clone()).unwrap();
    let original = receipt["results"][0].clone();
    assert!(project_font_write::validate_transition(&before, &after, &op, &original).is_ok());
    let mut forged = original.clone();
    forged["fontResources"]["samplingSha256"] = json!("0".repeat(64));
    assert!(project_font_write::validate_transition(&before, &after, &op, &forged).is_err());
    let mut unrelated = (*after).clone();
    unrelated.name = "Unrelated collateral".into();
    assert!(project_font_write::validate_transition(&before, &unrelated, &op, &original).is_err());
}

#[test]
fn project_font_editing_capability_requires_actual_independent_native_stop_transport() {
    let mut s = fixture();
    let before = state(&s);
    assert_eq!(read(&mut s, 91100)["capabilities"]["projectFontReplacementEditing"], false);
    s.agent_operation_control().enable_transport();
    assert_eq!(read(&mut s, 91101)["capabilities"]["projectFontReplacementEditing"], true);
    assert_eq!(state(&s), before);
}
