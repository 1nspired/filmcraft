use super::tests::{read, token};
use crate::Session;
use filmcraft_project::{ItemId, ParamValue};
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

fn setup() -> (Session, ItemId) {
    let mut s = super::tests::setup();
    let root = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).sequence_mut(root).unwrap().caption_tracks.clear();
    let sources: BTreeSet<_> = s.active_sequence().unwrap().all_tracks().flat_map(|t| t.items.iter().map(|c| c.item.0)).collect();
    for id in sources {
        s.execute(
            "agent.bindSource",
            json!({"item":id,"binding":{"projectId":"p_transfer","assetId":format!("a_{id}"),"mediaId":"original","role":"original"}}),
        )
        .unwrap();
    }
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Human active sibling","video":1,"audio":1})).unwrap();
    let child = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project).item_mut(child).unwrap().metadata.insert(super::sequences::KEY.into(), scope(root).to_string());
    (s, root)
}
fn scope(root: ItemId) -> Value {
    json!({"projectId":"p_transfer","anchorSequence":root.0})
}
fn destination(s: &mut Session, root: ItemId, n: usize) -> Value {
    read(s, n);
    s.execute("agent.readCaptionDestination", json!({"token":token(n),"scope":scope(root),"sequence":root.0})).unwrap()
}
fn request(root: ItemId, n: usize, basis: &Value) -> Value {
    let source = "{\"captions\":[\"Original words\"],\"confidence\":0.2,\"low_conf\":true}";
    use sha2::{Digest, Sha256};
    json!({"token":token(n),"label":"Exact original caption transfer","exact":true,"operations":[{"command":"agent.importCaptions","params":{
        "scope":scope(root),"beforeLayoutSha256":basis["layoutSha256"],"beforeCaptionSha256":basis["captionSha256"],"beforeArchiveSha256":basis["archiveSha256"],
        "sourceSha256":format!("{:x}",Sha256::digest(source.as_bytes())),"data":{"sequence":root.0,"sourceDocument":source,"tracks":[{
            "name":"Original captions","language":"und","format":"Subtitle","cues":[{"start":TICKS_PER_SECOND/1000*50,"end":TICKS_PER_SECOND/1000*71,"text":"Original words","speaker":"Groom","cueId":"original-20","settings":"align:start"}]}]}}}]})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal})
}

#[test]
fn guarded_caption_transfer_targets_inactive_root_and_matches_human_import_history_and_reopen() {
    let (mut s, root) = setup();
    let basis = destination(&mut s, root, 35000);
    let batch = request(root, 35000, &basis);
    let before = (*s.project).clone();
    let editor = serde_json::to_value(&s.state).unwrap();
    let undo = s.history.undo.len();
    let active = s.state.active_sequence;
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute("captions.importData", batch["operations"][0]["params"]["data"].clone()).unwrap();
    let receipt = s.execute("agent.apply", batch.clone()).unwrap();
    assert_eq!(receipt["action"], "caption_import");
    assert_eq!(receipt["results"][0]["humanCommand"], "captions.importData");
    assert_eq!(*s.project, *human.project);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), editor);
    assert_eq!(s.state.active_sequence, active);
    assert_eq!(s.history.undo.len(), undo + 1);
    let c = &s.project.sequence(root).unwrap().caption_tracks[0].captions[0];
    assert_eq!(c.duration.0, TICKS_PER_SECOND / 1000 * 21);
    assert_eq!(c.cue_id.as_deref(), Some("original-20"));
    let after = (*s.project).clone();
    assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt);
    assert_eq!(s.history.undo.len(), undo + 1);
    let current = destination(&mut s, root, 35001);
    assert_eq!(current["layoutSha256"], basis["layoutSha256"]);
    assert_eq!(current["captionSha256"], receipt["results"][0]["import"]["afterCaptionSha256"]);
    s.execute("agent.undo", json!({"token":token(35001),"commit":token(35000)})).unwrap();
    assert_eq!(*s.project, before);
    assert_eq!(s.state.active_sequence, active);
    read(&mut s, 35002);
    s.execute("agent.redo", json!({"token":token(35002),"commit":token(35000),"undo":token(35001)})).unwrap();
    assert_eq!(*s.project, after);
    assert_eq!(filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project, after);
}

#[test]
fn caption_destination_uses_existing_native_forward_clock_and_refuses_hold_remix_remap_reverse_or_foreign_sources() {
    for mode in ["hold", "speed", "reverse", "remix", "remap", "keyframes", "foreign", "unsafe-time"] {
        let (mut s, root) = setup();
        let p = Arc::make_mut(&mut s.project);
        let q = p.sequence_mut(root).unwrap();
        let c = &mut q.video_tracks[0].items[0];
        let item = c.item;
        match mode {
            "hold" => c.frame_hold = Some(Tick(0)),
            "speed" => c.speed = 0.5,
            "reverse" => c.reverse = true,
            "remix" => {
                let mut fx = filmcraft_project::effect::intrinsic_video().remove(0);
                fx.effect = "remix".into();
                c.effects.push(fx);
            }
            "remap" | "keyframes" => {
                let mut fx = filmcraft_project::effect::intrinsic_video().into_iter().find(|e| e.effect == "time_remap").unwrap();
                let value = fx.params.get_mut("speed").unwrap();
                if mode == "keyframes" {
                    value.value = ParamValue::Float(100.0);
                    value.keyframes.push(filmcraft_project::Keyframe::new(Tick(1), ParamValue::Float(100.0)));
                } else {
                    value.value = ParamValue::Float(50.0);
                }
                c.effects.push(fx);
            }
            "unsafe-time" => c.source_in = Tick(9_007_199_254_740_992),
            _ => {}
        }
        if mode == "foreign" {
            p.item_mut(item)
                .unwrap()
                .metadata
                .insert(super::BINDING_KEY.into(), json!({"projectId":"p_other","assetId":"a_other","mediaId":"original","role":"original"}).to_string());
        }
        read(&mut s, 35100);
        let before = state(&s);
        assert!(s.execute("agent.readCaptionDestination", json!({"token":token(35100),"scope":scope(root),"sequence":root.0})).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
    }
}

#[test]
fn malformed_stale_duplicate_and_mixed_caption_transfer_batches_preserve_human_redo() {
    for mode in ["layout", "caption", "source", "foreign", "destination", "negative", "outside", "missing-source", "missing-archive", "mixed", "inexact"] {
        let (mut s, root) = setup();
        s.execute("captions.newTrack", json!({"name":"Human redo"})).unwrap();
        s.undo().unwrap();
        let basis = destination(&mut s, root, 35200);
        let mut batch = request(root, 35200, &basis);
        let p = &mut batch["operations"][0]["params"];
        match mode {
            "layout" => p["beforeLayoutSha256"] = json!("0".repeat(64)),
            "caption" => p["beforeCaptionSha256"] = json!("0".repeat(64)),
            "source" => p["sourceSha256"] = json!("0".repeat(64)),
            "foreign" => p["scope"]["projectId"] = json!("p_other"),
            "destination" => p["data"]["sequence"] = json!(999999),
            "negative" => p["data"]["tracks"][0]["cues"][0]["start"] = json!(-1),
            "outside" => p["data"]["tracks"][0]["cues"][0]["end"] = json!(200 * TICKS_PER_SECOND),
            "missing-source" => {
                p["data"].as_object_mut().unwrap().remove("sourceDocument");
            }
            "missing-archive" => {
                p.as_object_mut().unwrap().remove("beforeArchiveSha256");
            }
            "mixed" => {
                let duplicate = batch["operations"][0].clone();
                batch["operations"].as_array_mut().unwrap().push(duplicate);
            }
            _ => batch["exact"] = json!(false),
        }
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
    }
    let (mut s, root) = setup();
    let basis = destination(&mut s, root, 35300);
    let batch = request(root, 35300, &basis);
    s.execute("captions.newTrack", json!({"name":"Later human work"})).unwrap();
    let before = state(&s);
    assert!(s.execute("agent.apply", batch).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn new_read_tickets_cannot_renew_an_old_caption_plan_after_source_or_archive_changes() {
    for mode in ["source", "archive"] {
        let (mut s, root) = setup();
        let basis = destination(&mut s, root, 35400);
        let mut batch = request(root, 35400, &basis);
        let p = Arc::make_mut(&mut s.project);
        if mode == "archive" {
            p.item_mut(root).unwrap().metadata.insert(crate::caption_import::ARCHIVE.into(), "{}".into());
        } else {
            let item = p.sequence(root).unwrap().video_tracks[0].items[0].item;
            p.item_mut(item)
                .unwrap()
                .metadata
                .insert(super::BINDING_KEY.into(), json!({"projectId":"p_transfer","assetId":"a_changed","mediaId":"original","role":"original"}).to_string());
        }
        read(&mut s, 35401);
        batch["token"] = json!(token(35401));
        let before = state(&s);
        assert!(s.execute("agent.apply", batch).is_err(), "{mode}");
        assert_eq!(state(&s), before);
    }
}

fn current_destination(s: &mut Session, root: ItemId, n: usize) -> Value {
    read(s, n);
    s.execute("agent.readCaptionDestination", json!({"token":token(n),"scope":scope(root),"sequence":root.0,"sourceKind":"current_mix"})).unwrap()
}
fn current_request(root: ItemId, n: usize, basis: &Value, replace: Option<u64>) -> Value {
    use sha2::{Digest, Sha256};
    let mut batch = request(root, n, basis);
    let p = &mut batch["operations"][0]["params"];
    let raw = json!({"kind":"poema-current-native-caption-source","version":1,"scope":{"project_id":"p_transfer","sequence_id":root.0},"story":{"status":"complete","speech":{"producer":"original-file-services"}}}).to_string();
    p["sourceKind"] = json!("current_mix");
    p["replaceTrack"] = json!(replace);
    p["sourceSha256"] = json!(format!("{:x}", Sha256::digest(raw.as_bytes())));
    p["data"]["sourceDocument"] = json!(raw);
    if let Some(track) = replace {
        let mut profile = basis["captionTemplates"].as_array().unwrap().iter().find(|v| v["id"] == track).unwrap().clone();
        profile.as_object_mut().unwrap().remove("id");
        profile["cues"] = p["data"]["tracks"][0]["cues"].clone();
        p["data"]["tracks"] = json!([profile]);
    }
    batch
}

#[test]
fn current_mixed_caption_import_supports_retimed_unannotated_cut_and_matches_shared_human_import() {
    let (mut s, root) = setup();
    s.state.active_sequence = Some(root);
    Arc::make_mut(&mut s.project).sequence_mut(root).unwrap().video_tracks[0].items[0].speed = 2.0;
    // Current mixed words do not pretend an original component annotation exists.
    let basis = current_destination(&mut s, root, 35500);
    assert_eq!(basis["currentMix"], true);
    assert!(basis.get("nativeForwardClocks").is_none());
    let batch = current_request(root, 35500, &basis, None);
    let before = (*s.project).clone();
    let undo = s.history.undo.len();
    let mut human = Session::new(s.services.clone());
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.media = s.media.clone();
    human.execute("captions.importData", batch["operations"][0]["params"]["data"].clone()).unwrap();
    let receipt = s.execute("agent.apply", batch.clone()).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(&human.project).unwrap());
    assert_eq!(receipt["results"][0]["sourceKind"], "current_mix");
    assert_eq!(s.history.undo.len(), undo + 1);
    assert_eq!(s.execute("agent.apply", batch).unwrap(), receipt);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(before).unwrap());
}

#[test]
fn current_caption_replacement_preserves_style_flags_position_other_tracks_and_one_native_history_step() {
    let (mut s, root) = setup();
    s.state.active_sequence = Some(root);
    s.execute("captions.importData",json!({"sequence":root.0,"tracks":[{"name":"Human Japanese","language":"ja","format":"Subtitle","enabled":false,"syncLock":false,"blocks":["NOTE human metadata"],"cues":[{"start":0,"end":TICKS_PER_SECOND,"text":"Human words"}]},{"name":"Other captions","language":"en","format":"Subtitle","cues":[{"start":0,"end":TICKS_PER_SECOND,"text":"Leave this"}]}]})).unwrap();
    let old = s.active_sequence().unwrap().caption_tracks.clone();
    let basis = current_destination(&mut s, root, 35501);
    let batch = current_request(root, 35501, &basis, Some(old[0].id.0));
    let before = (*s.project).clone();
    let undo = s.history.undo.len();
    let receipt = s.execute("agent.apply", batch).unwrap();
    let tracks = &s.active_sequence().unwrap().caption_tracks;
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].name, old[0].name);
    assert_eq!(tracks[0].style, old[0].style);
    assert_eq!(tracks[0].language, "ja");
    assert!(!tracks[0].enabled);
    assert!(!tracks[0].sync_lock);
    assert_eq!(tracks[0].vtt_blocks, old[0].vtt_blocks);
    assert_eq!(tracks[1], old[1]);
    assert_ne!(tracks[0].id, old[0].id);
    assert_eq!(receipt["results"][0]["replacedTrack"], old[0].id.0);
    assert_eq!(s.history.undo.len(), undo + 1);
    let new_basis = current_destination(&mut s, root, 35502);
    assert_eq!(new_basis["layoutSha256"], basis["layoutSha256"]);
    assert_eq!(new_basis["captionSha256"], receipt["results"][0]["import"]["afterCaptionSha256"]);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(serde_json::to_value(&s.project).unwrap(), serde_json::to_value(before).unwrap());
}

#[test]
fn current_caption_archive_policy_foreign_replacement_and_locked_or_changed_style_refuse_atomically() {
    let (mut s, root) = setup();
    s.state.active_sequence = Some(root);
    s.execute("captions.newTrack", json!({"name":"Human","language":"en"})).unwrap();
    let track = s.active_sequence().unwrap().caption_tracks[0].id.0;
    let basis = current_destination(&mut s, root, 35503);
    let batch = current_request(root, 35503, &basis, Some(track));
    let before = state(&s);
    for mode in ["unknown", "missing_policy", "foreign", "style", "archive"] {
        let mut bad = batch.clone();
        let p = &mut bad["operations"][0]["params"];
        match mode {
            "unknown" => p["sourceKind"] = json!("invented"),
            "missing_policy" => {
                p.as_object_mut().unwrap().remove("replaceTrack");
            }
            "foreign" => p["replaceTrack"] = json!(999999),
            "style" => p["data"]["tracks"][0]["name"] = json!("Overwrote human style"),
            _ => {
                p["data"]["sourceDocument"] = json!("{}");
                use sha2::{Digest, Sha256};
                p["sourceSha256"] = json!(format!("{:x}", Sha256::digest(b"{}")));
            }
        }
        assert!(s.execute("agent.apply", bad).is_err(), "{mode}");
        assert_eq!(state(&s), before, "{mode}");
    }
    Arc::make_mut(&mut s.project).sequence_mut(root).unwrap().caption_tracks[0].locked = true;
    let basis = current_destination(&mut s, root, 35504);
    let bad = current_request(root, 35504, &basis, Some(track));
    let before = state(&s);
    assert!(s.execute("agent.apply", bad).is_err());
    assert_eq!(state(&s), before);
}

#[test]
fn current_caption_replacement_restores_nonfirst_position_and_preserves_surrounding_tracks() {
    let (mut s, root) = setup();
    s.state.active_sequence = Some(root);
    s.execute("captions.importData",json!({"sequence":root.0,"tracks":[{"name":"Before","language":"en","format":"Subtitle","cues":[]},{"name":"Replace here","language":"ja","format":"Subtitle","cues":[]},{"name":"After","language":"en","format":"Subtitle","cues":[]}]})).unwrap();
    let old = s.active_sequence().unwrap().caption_tracks.clone();
    let basis = current_destination(&mut s, root, 35505);
    let batch = current_request(root, 35505, &basis, Some(old[1].id.0));
    let receipt = s.execute("agent.apply", batch).unwrap();
    let tracks = &s.active_sequence().unwrap().caption_tracks;
    assert_eq!(tracks[0], old[0]);
    assert_eq!(tracks[2], old[2]);
    assert_eq!(tracks[1].name, old[1].name);
    assert_eq!(tracks[1].language, old[1].language);
    assert_ne!(tracks[1].id, old[1].id);
    assert_eq!(receipt["results"][0]["humanCommands"], json!(["sequence.removeTracks", "captions.importData", "sequence.reorderTrack"]));
    let next = current_destination(&mut s, root, 35506);
    assert_eq!(next["captionSha256"], receipt["results"][0]["import"]["afterCaptionSha256"]);
}
