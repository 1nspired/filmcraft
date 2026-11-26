use super::*;
use crate::Session;
use crate::agent_bridge::{
    self,
    tests::{read, token},
};
use filmcraft_project::MediaRef;
use serde_json::json;
use std::{fs, path::PathBuf, sync::Arc};

struct Fixture {
    s: Session,
    file: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
    }
}
fn fixture() -> Fixture {
    let mut entropy = [0u8; 16];
    getrandom::fill(&mut entropy).unwrap();
    let file = std::env::temp_dir().join(format!("filmcraft-document-{:x}.bin", u128::from_le_bytes(entropy)));
    fs::write(&file, vec![39u8; 4096]).unwrap();
    let mut s = agent_bridge::tests::setup();
    let source = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    for item in Arc::make_mut(&mut s.project).items.values_mut().filter(|i| i.as_media().is_some()) {
        item.metadata.insert(
            agent_bridge::BINDING_KEY.into(),
            json!({"projectId":"p_document","assetId":format!("a_{}",item.id.0),"mediaId":"original","role":"original"}).to_string(),
        );
    }
    Arc::make_mut(&mut s.project).item_mut(source).unwrap().as_media_mut().unwrap().media = MediaRef::File { path: file.to_str().unwrap().into() };
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].id;
    s.execute("effects.apply", json!({"clips":[clip.0],"effect":"gaussian_blur"})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    Fixture { s, file }
}
fn request(s: &mut Session, doc: &Value, family: &str) -> (Value, &'static str, Value) {
    let sid = s.state.active_sequence.unwrap();
    let scope = json!({"projectId":"p_document","anchorSequence":sid.0});
    let (command, params, human, human_params) = match family {
        "track" => {
            let page = s.execute("agent.readTracks", json!({"token":doc["token"],"scope":scope,"offset":0,"limit":1,"projection":true})).unwrap();
            (
                "agent.track",
                json!({"scope":scope,"beforeSha256":page["sha256"],"edit":{"type":"add","video":1,"audio":0,"submix":0,"video_after":null,"audio_after":null,"submix_after":null,"audio_channels":"Stereo","submix_channels":"Stereo"}}),
                "sequence.addTracks",
                json!({"video":1,"audio":0,"submix":0,"videoAfter":0,"audioAfter":0,"submixAfter":0,"audioType":"Stereo","submixType":"Stereo"}),
            )
        }
        "marker" | "source_marker" => {
            let source = family == "source_marker";
            let target = if source { json!({"kind":"source","item":s.state.source_item.unwrap().0}) } else { json!({"kind":"sequence"}) };
            let page =
                s.execute("agent.readMarkers", json!({"token":doc["token"],"scope":scope,"target":target,"offset":0,"limit":1,"projection":true})).unwrap();
            (
                "agent.marker",
                json!({"scope":scope,"beforeSha256":page["sha256"],"target":target,"edit":{"type":"add","kind":"comment","position":{"ticks":0},"duration":{"ticks":0},"name":"Document note","comment":"Authored","color":"Green"}}),
                "markers.add",
                json!({"target":if source {"source"}else{"program"},"time":0,"name":"Document note","comment":"Authored","color":"Green"}),
            )
        }
        "caption" => (
            "agent.caption",
            json!({"scope":scope,"beforeSha256":doc["captionState"]["sha256"],"edit":{"type":"create_track","name":"Document captions","language":"en","format":"Subtitle"}}),
            "captions.newTrack",
            json!({"name":"Document captions","language":"en","format":"Subtitle"}),
        ),
        "parameter" => {
            let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
            let effect = c.effects.iter().find(|e| e.effect == "gaussian_blur").unwrap();
            (
                "agent.setParameter",
                json!({"scope":scope,"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"effectId":effect.instance_id,"maskId":null,"param":"blurriness","value":{"Float":27},"frame":0,"keyframe":false}),
                "effects.setParam",
                json!({"clip":c.id.0,"effect":c.effects.iter().position(|e|e.effect=="gaussian_blur").unwrap(),"param":"blurriness","value":27}),
            )
        }
        "effect_stack" => {
            let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
            let effect = c.effects.iter().find(|e| e.effect == "gaussian_blur").unwrap();
            (
                "agent.editEffectStack",
                json!({"scope":scope,"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"beforeStackSha256":effect_stack::digest(&c.effects).unwrap(),
                    "edit":{"type":"set_enabled","effectId":effect.instance_id,"enabled":false}}),
                "effects.toggleEnabled",
                json!({"clip":c.id.0,"index":c.effects.iter().position(|e|e.effect=="gaussian_blur").unwrap()}),
            )
        }
        "finish" => {
            let c = &s.active_sequence().unwrap().audio_tracks[0].items[0];
            let target = json!({"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id});
            (
                "agent.finish",
                json!({"scope":scope,"edit":{"type":"gain","clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"targets":[target],"mode":"set","db":-7}}),
                "clip.audioGain",
                json!({"clips":[c.id.0],"mode":"set","db":-7}),
            )
        }
        _ => panic!("test family"),
    };
    (
        json!({"token":doc["token"],"label":"Exact document gesture","exact":true,"authority":"document_state","operations":[{"command":command,"params":params}]}),
        human,
        human_params,
    )
}
fn project_again(s: &mut Session, n: usize, basis: usize) -> Value {
    s.execute("agent.projectDocument", json!({"token":token(n),"basis":token(basis),"mediaScope":{"sequences":[s.state.active_sequence.unwrap().0]}})).unwrap()
}
#[test]
fn document_families_match_human_services_and_keep_source_witnesses_unrenewed_through_offline_owned_undo_redo() {
    for (index, family) in ["track", "marker", "source_marker", "caption", "parameter", "finish", "effect_stack"].iter().enumerate() {
        let mut f = fixture();
        if *family == "source_marker" {
            let source = f.s.active_sequence().unwrap().video_tracks[0].items[0].item;
            f.s.state.source_item = Some(source);
            f.s.state.source_history = vec![source];
        }
        let n = 50000 + index * 100;
        let doc = read(&mut f.s, n);
        let (batch, command, params) = request(&mut f.s, &doc, family);
        let before = f.s.project.clone();
        let witness = f.s.agent_bridge.media.clone().unwrap();
        let mut human = Session::new(f.s.services.clone());
        human.project = f.s.project.clone();
        human.media = f.s.media.clone();
        human.state = f.s.state.clone();
        human.execute(command, params).unwrap();
        fs::remove_file(&f.file).unwrap();
        let mut legacy = batch.clone();
        legacy.as_object_mut().unwrap().remove("authority");
        assert!(f.s.execute("agent.apply", legacy).is_err(), "legacy media publication remains guarded: {family}");
        let receipt = f.s.execute("agent.apply", batch.clone()).unwrap();
        assert_eq!(receipt["authority"], "document_state");
        assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&human.project).unwrap(), "{family}");
        assert!(Arc::ptr_eq(&witness, f.s.agent_bridge.media.as_ref().unwrap()));
        assert_eq!(f.s.execute("agent.apply", batch.clone()).unwrap(), receipt);
        let mut borrowed = batch.clone();
        borrowed.as_object_mut().unwrap().remove("authority");
        assert!(f.s.execute("agent.apply", borrowed).is_err(), "a document receipt cannot reconcile another media request");
        let after = f.s.project.clone();
        let projected = project_again(&mut f.s, n + 1, n);
        assert_eq!(projected["documentProjection"]["sourceValidation"], "not-performed");
        assert!(f.s.execute("agent.validateSnapshot", json!({"token":token(n+1)})).is_err());
        let undone = f.s.execute("agent.undo", json!({"token":token(n+1),"commit":token(n)})).unwrap();
        assert_eq!(undone["authority"], "document_state");
        assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&before).unwrap());
        project_again(&mut f.s, n + 2, n + 1);
        let redone = f.s.execute("agent.redo", json!({"token":token(n+2),"commit":token(n),"undo":token(n+1)})).unwrap();
        assert_eq!(redone["authority"], "document_state");
        assert_eq!(project_audit::bytes(&f.s.project).unwrap(), project_audit::bytes(&after).unwrap());
        assert!(Arc::ptr_eq(&witness, f.s.agent_bridge.media.as_ref().unwrap()));
    }
}
#[test]
fn document_authority_rejects_unknown_mixed_nonexact_and_media_consuming_batches_without_mutation() {
    let mut f = fixture();
    let doc = read(&mut f.s, 51000);
    let (valid, _, _) = request(&mut f.s, &doc, "track");
    let before = f.s.project.clone();
    for command in ["agent.admitSource", "agent.bindSource", "agent.previewSource", "timeline.razor", "agent.assembleSequence", "file.import"] {
        let mut b = valid.clone();
        b["operations"][0]["command"] = json!(command);
        assert!(f.s.execute("agent.apply", b).is_err(), "{command}");
    }
    for authority in [json!(null), json!(true), json!("media"), json!("document_state_extra")] {
        let mut b = valid.clone();
        b["authority"] = authority;
        assert!(f.s.execute("agent.apply", b).is_err());
    }
    let mut b = valid.clone();
    b["exact"] = json!(false);
    assert!(f.s.execute("agent.apply", b).is_err());
    let mut b = valid.clone();
    b["operations"].as_array_mut().unwrap().push(valid["operations"][0].clone());
    assert!(f.s.execute("agent.apply", b).is_err());
    assert!(f.s.execute("agent.prepareApply", json!({"job":token(51001),"batch":valid})).is_err());
    assert!(Arc::ptr_eq(&before, &f.s.project));
    assert!(mark(&mut json!(false), Authority::DocumentState).is_err());
    f.s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(51000)).unwrap().preparation = Some(token(51009));
    assert!(f.s.execute("agent.apply", valid).is_err(), "an attempted media preparation cannot be converted to a document publication");
    assert!(Arc::ptr_eq(&before, &f.s.project));
}
#[test]
fn document_authority_refuses_changed_context_human_history_and_source_declaration_collateral() {
    let mut f = fixture();
    let doc = read(&mut f.s, 52000);
    let (batch, _, _) = request(&mut f.s, &doc, "track");
    f.s.execute("timeline.setTrack", json!({"track":"V1","name":"Human"})).unwrap();
    let before = f.s.project.clone();
    assert!(f.s.execute("agent.apply", batch).is_err());
    assert!(Arc::ptr_eq(&before, &f.s.project));
    let mut changed = (*before).clone();
    let item = changed.items.values_mut().find(|i| i.as_media().is_some()).unwrap();
    item.as_media_mut().unwrap().offline = true;
    assert!(transition(&before, &changed).is_err());
}

#[test]
fn document_authority_keeps_stop_human_interleave_and_receipt_domain_guards() {
    let mut f = fixture();
    let doc = read(&mut f.s, 53000);
    let (batch, _, _) = request(&mut f.s, &doc, "track");
    let before = f.s.project.clone();
    f.s.agent_bridge.operations.cancel(&json!({"token":token(53000)})).unwrap();
    assert!(f.s.execute("agent.apply", batch).is_err());
    assert!(Arc::ptr_eq(&before, &f.s.project));
    let doc = read(&mut f.s, 53001);
    let (batch, _, _) = request(&mut f.s, &doc, "track");
    f.s.execute("agent.apply", batch).unwrap();
    f.s.execute("timeline.setTrack", json!({"track":"V1","name":"Later human"})).unwrap();
    project_again(&mut f.s, 53002, 53001);
    let human = f.s.project.clone();
    assert!(f.s.execute("agent.undo", json!({"token":token(53002),"commit":token(53001)})).is_err());
    assert!(Arc::ptr_eq(&human, &f.s.project));
    let commit = f.s.agent_bridge.tickets.iter_mut().find(|t| t.token == token(53001)).unwrap();
    commit.committed.as_mut().unwrap().1.as_object_mut().unwrap().remove("authority");
    assert!(committed(commit).is_err());
}
