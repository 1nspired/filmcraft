use super::assembly_tests::fixture;
use super::tests::{exact_batch, read, token};
use filmcraft_project::ParamValue;
use filmcraft_time::Tick;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn setup() -> crate::Session {
    let (mut s, _) = fixture();
    let seq = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    for track in q.all_tracks_mut() {
        for (n, clip) in track.items.iter_mut().enumerate() {
            clip.start = rate.tick_of([10, 40, 60][n]);
            clip.duration = rate.tick_of(if n == 0 { 20 } else { 12 });
            clip.source_in = rate.tick_of(100);
            if n == 0 {
                clip.identity.as_mut().unwrap().annotations.insert("editor_note".into(), "Keep this idea".into());
            }
        }
    }
    let a = &mut q.audio_tracks[0].items[0];
    a.start = rate.tick_of(8);
    a.duration = rate.tick_of(24);
    a.source_in = rate.tick_of(200);
    a.gain_db = -3.0;
    q.check().unwrap();
    s.history.undo.clear();
    s.history.redo.clear();
    s.journal.clear();
    s.events.clear();
    s
}
pub(super) fn operation(s: &crate::Session) -> Value {
    let q = s.active_sequence().unwrap();
    let c = &q.video_tracks[0].items[0];
    let rate = s.sequence_rate();
    json!({"command":"agent.condense","params":{"projectId":"p_test","receiptSha256":"c".repeat(64),"version":1,
        "passages":[{"clip":c.id.0,"identity":c.identity.as_ref().unwrap().id,"sourceIn":c.source_in.0,
        "sourceOut":c.source_out().0,"start":c.start.0,"kept":[[rate.tick_of(102).0,rate.tick_of(108).0],[rate.tick_of(110).0,rate.tick_of(118).0]]}]}})
}
fn rejected(s: &mut crate::Session, op: Value) {
    rejected_at(s, op, 9100)
}
fn rejected_at(s: &mut crate::Session, op: Value, n: usize) {
    if filmcraft_project::clip_identity::validate_project(&s.project).is_ok() {
        s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
        s.undo().unwrap();
        s.history.merge_key = Some("existing human gesture".into());
    }
    let before = (
        s.project.clone(),
        s.revision,
        s.saved_revision,
        s.history.undo.clone(),
        s.history.redo.clone(),
        s.history.merge_key.clone(),
        s.journal.clone(),
        serde_json::to_value(&s.state).unwrap(),
        s.events.len(),
    );
    match s.execute("agent.snapshot", json!({"token":token(n)})) {
        Ok(_) => assert!(s.execute("agent.apply", exact_batch(n, vec![op])).is_err()),
        Err(e) => assert!(e.to_string().contains("persistent identity")),
    }
    assert!(Arc::ptr_eq(&s.project, &before.0));
    assert_eq!((s.revision, s.saved_revision), (before.1, before.2));
    for (a, b) in [(&s.history.undo, &before.3), (&s.history.redo, &before.4)] {
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b).all(|((al, ap), (bl, bp))| al == bl && Arc::ptr_eq(ap, bp)), "rejection changed a native history snapshot");
    }
    assert_eq!(s.history.merge_key, before.5);
    assert_eq!(s.journal, before.6);
    assert_eq!(serde_json::to_value(&s.state).unwrap(), before.7);
    assert_eq!(s.events.len(), before.8);
}

#[test]
fn native_condense_full_restoration_preserves_later_human_finishing_and_unrelated_work() {
    let mut s = setup();
    let original = s.project.clone();
    let seq = s.state.active_sequence.unwrap();
    let leader = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
    let op = operation(&s);
    read(&mut s, 9300);
    s.execute("agent.apply", exact_batch(9300, vec![op])).unwrap();
    s.edit_sequence("Later human finishing", |q, _, _| {
        q.video_tracks[0].items[0].name = "Human graded first part".into();
        q.video_tracks[0].items[1].name = "Human graded second part".into();
        q.video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(73.0);
        q.video_tracks[0].items[1].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(91.0);
        q.video_tracks[0].items[1].identity.as_mut().unwrap().annotations.insert("human_note".into(), "Keep my finishing".into());
        q.audio_tracks[0].items[1].gain_db = -7.0;
        q.video_tracks[0].items[2].name = "Unrelated human note".into();
        Ok(())
    })
    .unwrap();
    let human = s.project.clone();
    read(&mut s, 9301);
    let restore = json!({"command":"agent.restoreCondense","params":{"projectId":"p_test","identity":leader,"receiptSha256":"c".repeat(64)}});
    s.execute("agent.apply", exact_batch(9301, vec![restore])).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items[0].name, "Human graded first part");
    assert_eq!(q.video_tracks[0].items[1].name, "Human graded second part");
    assert_eq!(q.video_tracks[0].items[2].name, "Unrelated human note");
    assert_eq!(q.audio_tracks[0].items[1].gain_db, -7.0);
    assert_eq!(q.video_tracks[0].items[0].effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value, ParamValue::Float(73.0));
    assert_eq!(q.video_tracks[0].items[1].effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value, ParamValue::Float(91.0));
    assert_eq!(q.video_tracks[0].items[1].identity.as_ref().unwrap().annotations["human_note"], "Keep my finishing");
    let old = original.sequence(seq).unwrap();
    for (current, before) in [(&q.video_tracks[0], &old.video_tracks[0]), (&q.audio_tracks[0], &old.audio_tracks[0])] {
        let end = before.items[0].end();
        let parts: Vec<_> = current.items.iter().filter(|c| c.start < end).collect();
        assert_eq!(parts.first().unwrap().start, before.items[0].start);
        assert_eq!(parts.last().unwrap().end(), end);
        assert_eq!(parts.first().unwrap().source_in, before.items[0].source_in);
        assert_eq!(parts.last().unwrap().source_out(), before.items[0].source_out());
        assert!(parts.windows(2).all(|w| w[0].end() == w[1].start && w[0].source_out() == w[1].source_in));
        assert!(parts.iter().all(|c| c.link == before.items[0].link));
    }
    read(&mut s, 9302);
    s.execute("agent.undo", json!({"token":token(9302),"commit":token(9301)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &human));
}

#[test]
fn native_condense_cannot_cut_another_passages_unqualified_jl_tail() {
    let mut s = setup();
    let rate = s.sequence_rate();
    let id = s.state.active_sequence.unwrap();
    let q = Arc::make_mut(&mut s.project).sequence_mut(id).unwrap();
    // The first passage's recorder continues through the second passage. Its
    // own R5 receipt only heard the first leader's interval, not this tail.
    q.audio_tracks[0].items[0].duration = rate.tick_of(60);
    // Keep the synthetic timeline valid: remove the other audio on that lane.
    q.audio_tracks[0].items.truncate(1);
    let second = q.video_tracks[0].items[1].clone();
    let mut op = operation(&s);
    op["params"]["passages"].as_array_mut().unwrap().push(json!({
        "clip":second.id.0,"identity":second.identity.as_ref().unwrap().id,"sourceIn":second.source_in.0,
        "sourceOut":second.source_out().0,"start":second.start.0,
        "kept":[[rate.tick_of(100).0,rate.tick_of(104).0],[rate.tick_of(106).0,rate.tick_of(112).0]]
    }));
    rejected(&mut s, op);
}

fn restore_op(identity: &str) -> Value {
    json!({"command":"agent.restoreCondense","params":{"projectId":"p_test","identity":identity,"receiptSha256":"c".repeat(64)}})
}

fn choice_op(s: &crate::Session, identity: &str, version: u8, kept: &[[i64; 2]]) -> Value {
    let rate = s.sequence_rate();
    json!({"command":"agent.chooseCondense","params":{"projectId":"p_test","receiptSha256":"c".repeat(64),"version":version,
        "passages":[{"identity":identity,"sourceIn":rate.tick_of(100).0,"sourceOut":rate.tick_of(120).0,
        "kept":kept.iter().map(|[a,z]|[rate.tick_of(*a).0,rate.tick_of(*z).0]).collect::<Vec<_>>()}]}})
}

#[test]
fn canonical_condense_full_tight_choices_round_trip_without_aliasing_raw_native_span_or_losing_human_finishing() {
    let mut s = setup();
    let sid = s.state.active_sequence.unwrap();
    let rate = s.sequence_rate();
    let identity = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
    let mut op = operation(&s);
    op["params"]["version"] = json!(4);
    op["params"]["passages"][0]["kept"] = json!([[rate.tick_of(101).0, rate.tick_of(119).0]]);
    read(&mut s, 24900);
    s.execute("agent.apply", exact_batch(24900, vec![op])).unwrap();
    s.edit_sequence("Human finishing before canonical choice", |q, _, _| {
        q.video_tracks[0].items[0].name = "Human canonical finish".into();
        q.video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(79.0);
        q.audio_tracks[0].items[0].gain_db = -8.0;
        Ok(())
    })
    .unwrap();
    let full = s.project.clone();
    let catalog = super::condense_restore::catalog(&s, sid).unwrap();
    assert_eq!(catalog[0]["selection"], "full");
    assert_eq!(catalog[0]["recordedState"], "condensed");
    assert_eq!(catalog[0]["sourceIn"], rate.tick_of(100).0);
    assert_eq!(catalog[0]["sourceOut"], rate.tick_of(120).0);
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    assert_eq!(c.source_in, rate.tick_of(101));
    assert_eq!(c.source_out(), rate.tick_of(119));
    let tight = choice_op(&s, &identity, 5, &[[102, 106], [110, 118]]);
    read(&mut s, 24901);
    s.execute("agent.apply", exact_batch(24901, vec![tight])).unwrap();
    assert_eq!(super::condense_restore::catalog(&s, sid).unwrap()[0]["selection"], "tight");
    let tight_project = s.project.clone();
    read(&mut s, 24902);
    s.execute("agent.undo", json!({"token":token(24902),"commit":token(24901)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &full));
    s.redo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &tight_project));
    let encoded = filmcraft_format::encode(&s.project, false);
    let decoded = filmcraft_format::decode(&encoded).unwrap().project;
    let mut reopened = crate::Session::new(s.services.clone());
    reopened.project = Arc::new(decoded);
    reopened.state = s.state.clone();
    assert_eq!(super::condense_restore::catalog(&reopened, sid).unwrap()[0]["selection"], "tight");
    let full_choice = choice_op(&reopened, &identity, 4, &[[101, 119]]);
    read(&mut reopened, 24903);
    reopened.execute("agent.apply", exact_batch(24903, vec![full_choice])).unwrap();
    let q = reopened.active_sequence().unwrap();
    let pieces: Vec<_> = q.video_tracks[0].items.iter().filter(|c| c.name == "Human canonical finish").collect();
    assert!(!pieces.is_empty());
    assert!(pieces.iter().all(|c| c.source_in >= rate.tick_of(101) && c.source_out() <= rate.tick_of(119)));
    assert_eq!(pieces.iter().map(|c| c.duration).fold(Tick::ZERO, |n, d| n + d), rate.tick_of(18));
    assert!(pieces.iter().all(|c| c.effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value == ParamValue::Float(79.0)));
    assert!(q.audio_tracks[0].items.iter().any(|c| c.gain_db == -8.0));
    assert_eq!(super::condense_restore::catalog(&reopened, sid).unwrap()[0]["selection"], "full");
}

#[test]
fn canonical_condense_choice_rejects_unrecognized_selectors_without_publication_or_human_redo_loss() {
    let mut s = setup();
    let mut op = operation(&s);
    op["params"]["version"] = json!(6);
    rejected_at(&mut s, op, 24910);
    let (mut s, identity) = condensed();
    let op = choice_op(&s, &identity, 6, &[[102, 106]]);
    rejected_at(&mut s, op, 24911);
}

#[test]
fn native_condense_version_choice_preserves_hidden_human_finishing_and_immediate_undo() {
    let mut s = setup();
    let op = operation(&s);
    let identity = op["params"]["passages"][0]["identity"].as_str().unwrap().to_string();
    read(&mut s, 9700);
    s.execute("agent.apply", exact_batch(9700, vec![op])).unwrap();
    s.edit_sequence("Human finishing on the second telling", |q, _, _| {
        q.video_tracks[0].items[0].name = "First human finish".into();
        let c = &mut q.video_tracks[0].items[1];
        c.name = "Hidden human finish must return".into();
        c.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(91.0);
        c.identity.as_mut().unwrap().annotations.insert("human_note".into(), "Do not lose this grade".into());
        q.audio_tracks[0].items[1].gain_db = -7.0;
        q.video_tracks[0].items[2].name = "Unrelated human work".into();
        Ok(())
    })
    .unwrap();
    let human = s.project.clone();
    let choose = choice_op(&s, &identity, 2, &[[102, 106]]);
    read(&mut s, 9701);
    let undo = s.history.undo.len();
    s.execute("agent.apply", exact_batch(9701, vec![choose])).unwrap();
    assert_eq!(s.history.undo.len(), undo + 1);
    let q = s.active_sequence().unwrap();
    assert!(!q.video_tracks[0].items.iter().any(|c| c.name == "Hidden human finish must return"));
    assert!(q.video_tracks[0].items.iter().any(|c| c.name == "Unrelated human work"));
    let chosen = s.project.clone();
    read(&mut s, 9702);
    s.execute("agent.apply", exact_batch(9702, vec![restore_op(&identity)])).unwrap();
    let q = s.active_sequence().unwrap();
    let hidden = q.video_tracks[0].items.iter().find(|c| c.name == "Hidden human finish must return").unwrap();
    assert_eq!(hidden.effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value, ParamValue::Float(91.0));
    assert_eq!(hidden.identity.as_ref().unwrap().annotations["human_note"], "Do not lose this grade");
    assert!(q.audio_tracks[0].items.iter().any(|c| c.gain_db == -7.0));
    assert!(q.video_tracks[0].items.iter().any(|c| c.name == "Unrelated human work"));
    read(&mut s, 9703);
    s.execute("agent.undo", json!({"token":token(9703),"commit":token(9702)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &chosen));
    read(&mut s, 9704);
    s.execute("agent.undo", json!({"token":token(9704),"commit":token(9701)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &human));
}

#[test]
fn native_condense_version_choice_round_trips_hidden_finishing_through_save_reopen_and_recovery() {
    let (mut s, identity) = condensed();
    s.edit_sequence("Human hidden title", |q, _, _| {
        q.video_tracks[0].items[1].name = "Saved hidden human finish".into();
        Ok(())
    })
    .unwrap();
    let human = s.project.clone();
    let op = choice_op(&s, &identity, 2, &[[102, 106]]);
    read(&mut s, 9720);
    s.execute("agent.apply", exact_batch(9720, vec![op])).unwrap();
    let journal = s.journal.last().unwrap().clone();
    let mut recovery = crate::Session::new(s.services.clone());
    recovery.project = human;
    recovery.state = s.state.clone();
    assert_eq!(journal.0, "agent.replayBatch");
    recovery.execute(&journal.0, journal.1).unwrap();
    // Recovery regenerates random instance identities, just like ordinary
    // native Extract recovery. Compare the complete graph/metadata using the
    // corresponding native numeric IDs, retaining notes/protection/settings.
    fn normalized_recovery(p: &filmcraft_project::Project) -> Value {
        fn expand(v: &mut Value) {
            if let Some(text) = v.as_str()
                && let Ok(parsed) = serde_json::from_str::<Value>(text)
                && (parsed.is_object() || parsed.is_array())
            {
                *v = parsed;
            }
            match v {
                Value::Object(m) => {
                    for v in m.values_mut() {
                        expand(v);
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        expand(v);
                    }
                }
                _ => {}
            }
        }
        fn collect(v: &Value, m: &mut std::collections::BTreeMap<String, String>) {
            if let Some(o) = v.as_object() {
                let pair = o
                    .get("id")
                    .and_then(Value::as_u64)
                    .zip(o.get("identity").and_then(|i| i.get("id")).and_then(Value::as_str))
                    .or_else(|| o.get("clip").and_then(Value::as_u64).zip(o.get("identity").and_then(Value::as_str)));
                if let Some((n, id)) = pair {
                    m.insert(id.into(), format!("native-instance-{n}"));
                }
                for v in o.values() {
                    collect(v, m);
                }
            } else if let Some(a) = v.as_array() {
                for v in a {
                    collect(v, m);
                }
            }
        }
        fn replace(v: &mut Value, m: &std::collections::BTreeMap<String, String>) {
            match v {
                Value::String(s) => {
                    if let Some(id) = m.get(s) {
                        *s = id.clone();
                    }
                }
                Value::Object(o) => {
                    for v in o.values_mut() {
                        replace(v, m);
                    }
                }
                Value::Array(a) => {
                    for v in a {
                        replace(v, m);
                    }
                }
                _ => {}
            }
        }
        let mut v = serde_json::to_value(p).unwrap();
        expand(&mut v);
        let mut ids = std::collections::BTreeMap::new();
        collect(&v, &mut ids);
        replace(&mut v, &ids);
        v
    }
    assert_eq!(normalized_recovery(&recovery.project), normalized_recovery(&s.project));
    let directory = crate::test_fixture_root(std::env::var_os("FILMCRAFT_FIXTURES_DIR"), std::env::var("POEMA_FIXTURE_ROOT_REQUIRED").as_deref() == Ok("1"))
        .unwrap()
        .join(format!("hidden-version-{}-{}", std::process::id(), token(9720)));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generated-hidden-choice.fcproj");
    s.execute("file.saveAs", json!({"path":file})).unwrap();
    let mut loaded = crate::Session::new(s.services.clone());
    loaded.execute("file.open", json!({"path":file})).unwrap();
    read(&mut loaded, 9721);
    loaded.execute("agent.apply", exact_batch(9721, vec![restore_op(&identity)])).unwrap();
    assert!(loaded.active_sequence().unwrap().video_tracks[0].items.iter().any(|c| c.name == "Saved hidden human finish"));
    loaded.execute("file.save", json!({})).unwrap();
    let mut reopened = crate::Session::new(s.services.clone());
    reopened.execute("file.open", json!({"path":file})).unwrap();
    assert_eq!(serde_json::to_value(&*loaded.project).unwrap(), serde_json::to_value(&*reopened.project).unwrap());
}

#[test]
fn native_condense_version_choice_rejection_preserves_human_redo_and_never_qualifies_changed_audio_input() {
    for case in 0..12 {
        let (mut s, identity) = condensed();
        let mut op = choice_op(&s, &identity, 2, &[[102, 106]]);
        match case {
            0 => op["params"]["receiptSha256"] = json!("d".repeat(64)),
            1 => op["params"]["projectId"] = json!("foreign"),
            2 => op["params"]["passages"][0]["sourceOut"] = json!(s.sequence_rate().tick_of(119).0),
            3 => op["params"]["version"] = json!(0),
            4 => op["params"]["passages"][0]["kept"] = json!([[0, 1]]),
            5 => {
                let p = op["params"]["passages"][0].clone();
                op["params"]["passages"].as_array_mut().unwrap().push(p);
            }
            _ => {
                s.edit_sequence("Human changed the saved basis", |q, ctx, _| {
                    match case {
                        6 => q.video_tracks[0].items[0].duration -= Tick(1),
                        7 => q.audio_tracks[0].locked = true,
                        8 => q.video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("Human freeze".into()),
                        9 => q.audio_tracks[0].items[0].source_channels = vec![1],
                        10 => q.audio_tracks[0].muted = true,
                        _ => {
                            let mut c = q.video_tracks[0].items[0].clone();
                            c.id = filmcraft_project::ClipId(ctx.alloc());
                            let mut t = q.video_tracks[0].clone();
                            t.id = filmcraft_project::TrackId(ctx.alloc());
                            t.items = vec![c];
                            t.transitions.clear();
                            q.video_tracks.push(t);
                        }
                    }
                    Ok(())
                })
                .unwrap();
            }
        }
        rejected(&mut s, op);
    }
}

#[test]
fn native_condense_version_choice_rejects_a_repeated_choice_and_can_restore_the_complete_native_span() {
    let (mut s, identity) = condensed();
    let op = choice_op(&s, &identity, 2, &[[102, 106]]);
    read(&mut s, 9730);
    s.execute("agent.apply", exact_batch(9730, vec![op.clone()])).unwrap();
    rejected(&mut s, op);
    let full = choice_op(&s, &identity, 3, &[[100, 120]]);
    read(&mut s, 9731);
    let receipt = s.execute("agent.apply", exact_batch(9731, vec![full.clone()])).unwrap();
    assert_eq!(receipt["results"][0]["removedTicks"], 0);
    assert_eq!(receipt["results"][0]["restorations"][0]["recordedState"], "original_native_span");
    assert_eq!(receipt["results"][0]["joinTicks"], json!([]));
    rejected_at(&mut s, full, 9101);
}

#[test]
fn native_condense_version_choice_can_repeat_different_versions_without_flattening_hidden_partitions() {
    let (mut s, identity) = condensed();
    s.edit_sequence("Human finishing", |q, _, _| {
        q.video_tracks[0].items[1].name = "Keep the original second finish".into();
        Ok(())
    })
    .unwrap();
    for (n, (version, kept)) in [(2, vec![[102, 106]]), (3, vec![[111, 116]]), (1, vec![[102, 108], [110, 118]]), (2, vec![[102, 106]])].into_iter().enumerate()
    {
        let op = choice_op(&s, &identity, version, &kept);
        read(&mut s, 9740 + n);
        s.execute("agent.apply", exact_batch(9740 + n, vec![op])).unwrap();
        let q = s.active_sequence().unwrap();
        q.check().unwrap();
        assert_eq!(
            q.video_tracks[0].items.iter().filter(|c| c.source_in >= s.sequence_rate().tick_of(110) && c.name == "Keep the original second finish").count(),
            usize::from(version != 2)
        );
    }
    read(&mut s, 9749);
    s.execute("agent.apply", exact_batch(9749, vec![restore_op(&identity)])).unwrap();
    assert!(s.active_sequence().unwrap().video_tracks[0].items.iter().any(|c| c.name == "Keep the original second finish"));
}

#[test]
fn native_condense_version_choice_requires_one_exact_logical_batch_and_a_current_ticket() {
    for case in 0..3 {
        let (mut s, identity) = condensed();
        let op = choice_op(&s, &identity, 2, &[[102, 106]]);
        read(&mut s, 9750);
        let mut batch = exact_batch(9750, vec![op.clone()]);
        if case == 0 {
            batch["exact"] = json!(false);
        } else if case == 1 {
            batch["operations"].as_array_mut().unwrap().push(op);
        } else {
            s.edit_sequence("Later human note", |q, _, _| {
                q.video_tracks[0].items[2].name = "Not replaceable".into();
                Ok(())
            })
            .unwrap();
        }
        let before = (s.project.clone(), s.revision, s.history.undo.len(), s.journal.clone());
        assert!(s.execute("agent.apply", batch).is_err());
        assert!(Arc::ptr_eq(&s.project, &before.0));
        assert_eq!((s.revision, s.history.undo.len(), s.journal.clone()), (before.1, before.2, before.3));
    }
}

#[test]
fn native_condense_legacy_records_keep_their_inverse_but_cannot_invent_original_audio_qualification() {
    let (mut s, identity) = condensed();
    let seq = s.state.active_sequence.unwrap();
    let raw = Arc::make_mut(&mut s.project).item_mut(seq).unwrap().metadata.get_mut("poema.condense.restore.v1").unwrap();
    let mut metadata: Value = serde_json::from_str(raw).unwrap();
    metadata["version"] = json!(1);
    metadata["records"][0].as_object_mut().unwrap().remove("basis");
    *raw = serde_json::to_string(&metadata).unwrap();
    let op = choice_op(&s, &identity, 2, &[[102, 106]]);
    rejected(&mut s, op);
    read(&mut s, 9760);
    s.execute("agent.apply", exact_batch(9760, vec![restore_op(&identity)])).unwrap();
}

#[test]
fn native_condense_saved_choices_rebase_two_logical_passages_in_either_order_without_losing_human_work() {
    for reverse in [false, true] {
        let mut s = setup();
        let mut op = operation(&s);
        let q = s.active_sequence().unwrap();
        let c = &q.video_tracks[0].items[1];
        let rate = s.sequence_rate();
        let second = c.identity.as_ref().unwrap().id.clone();
        op["params"]["passages"].as_array_mut().unwrap().push(json!({"clip":c.id.0,"identity":second,"sourceIn":c.source_in.0,"sourceOut":c.source_out().0,
            "start":c.start.0,"kept":[[rate.tick_of(102).0,rate.tick_of(110).0]]}));
        let original_passages = op["params"]["passages"].as_array().unwrap().clone();
        read(&mut s, 9770);
        s.execute("agent.apply", exact_batch(9770, vec![op])).unwrap();
        s.edit_sequence("Both human finishes", |q, _, _| {
            for c in &mut q.video_tracks[0].items {
                c.name = format!("Human choice {}", c.id.0);
            }
            Ok(())
        })
        .unwrap();
        let human = s.project.clone();
        let mut passages: Vec<_> = original_passages
            .iter()
            .map(|b| {
                json!({"identity":b["identity"],"sourceIn":b["sourceIn"],"sourceOut":b["sourceOut"],
            "kept":[[rate.tick_of(102).0,rate.tick_of(106).0]]})
            })
            .collect();
        if reverse {
            passages.reverse();
        }
        let choose = json!({"command":"agent.chooseCondense","params":{"projectId":"p_test","receiptSha256":"c".repeat(64),"version":2,"passages":passages}});
        read(&mut s, 9771);
        let result = s.execute("agent.apply", exact_batch(9771, vec![choose])).unwrap();
        assert_eq!(result["results"][0]["condensed"], 2);
        assert_eq!(result["results"][0]["passageRanges"].as_array().unwrap().len(), 2);
        let q = s.active_sequence().unwrap();
        q.check().unwrap();
        assert_eq!(q.video_tracks[0].items.iter().filter(|c| c.duration == rate.tick_of(4)).count(), 2);
        assert!(q.video_tracks[0].items.iter().all(|c| c.name.starts_with("Human choice")));
        read(&mut s, 9772);
        s.execute("agent.undo", json!({"token":token(9772),"commit":token(9771)})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &human));
    }
}

#[test]
fn native_condense_does_not_claim_a_preexisting_sibling_as_a_new_extract_descendant() {
    let mut s = setup();
    s.state.linked_selection = true;
    let rate = s.sequence_rate();
    let id = s.active_sequence().unwrap().video_tracks[0].items[0].id.0;
    s.execute("timeline.razor", json!({"clip":id,"time":rate.tick_of(20).0})).unwrap();
    let q = s.active_sequence().unwrap();
    let sibling = q.video_tracks[0].items[1].clone();
    let mut op = operation(&s);
    op["params"]["passages"][0]["kept"] = json!([[rate.tick_of(102).0, rate.tick_of(108).0]]);
    read(&mut s, 9780);
    s.execute("agent.apply", exact_batch(9780, vec![op])).unwrap();
    let q = s.active_sequence().unwrap();
    let after = q.find_item(sibling.id).unwrap().1;
    assert_eq!((after.source_in, after.duration, after.identity.clone()), (sibling.source_in, sibling.duration, sibling.identity.clone()));
    let records = super::condense_restore::catalog(&s, s.state.active_sequence.unwrap()).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
}
fn condensed() -> (crate::Session, String) {
    let mut s = setup();
    let identity = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
    let op = operation(&s);
    read(&mut s, 9400);
    s.execute("agent.apply", exact_batch(9400, vec![op])).unwrap();
    (s, identity)
}

#[test]
fn native_restore_refuses_reshaped_locked_protected_missing_foreign_or_collateral_targets_without_history_loss() {
    for case in 0..7 {
        let (mut s, identity) = condensed();
        let mut op = restore_op(&identity);
        if case == 0 {
            op["params"]["receiptSha256"] = json!("d".repeat(64));
        } else if case == 1 {
            op["params"]["projectId"] = json!("foreign");
        } else {
            s.edit_sequence("Human intervened", |q, ctx, _| {
                match case {
                    2 => q.video_tracks[0].items[0].duration -= Tick(1),
                    3 => q.video_tracks[0].locked = true,
                    4 => q.video_tracks[0].items[0].identity.as_mut().unwrap().protection = Some("Human freeze".into()),
                    5 => {
                        q.video_tracks[0].items.remove(0);
                    }
                    _ => {
                        let mut c = q.video_tracks[0].items[0].clone();
                        c.id = filmcraft_project::ClipId(ctx.alloc());
                        let mut track = q.video_tracks[0].clone();
                        track.id = filmcraft_project::TrackId(ctx.alloc());
                        track.items = vec![c];
                        track.transitions.clear();
                        q.video_tracks.push(track);
                    }
                }
                Ok(())
            })
            .unwrap();
        }
        rejected(&mut s, op);
    }
}

#[test]
fn native_restore_independent_passages_in_either_order_preserves_other_versions_and_current_anchors() {
    for reverse in [false, true] {
        let mut s = setup();
        let rate = s.sequence_rate();
        let q = s.active_sequence().unwrap();
        let first = q.video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
        let second = q.video_tracks[0].items[1].clone();
        let second_identity = second.identity.as_ref().unwrap().id.clone();
        let mut op = operation(&s);
        op["params"]["passages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"clip":second.id.0,"identity":second_identity,"sourceIn":second.source_in.0,"sourceOut":second.source_out().0,"start":second.start.0,
            "kept":[[rate.tick_of(100).0,rate.tick_of(104).0],[rate.tick_of(106).0,rate.tick_of(112).0]]}));
        read(&mut s, 9410);
        s.execute("agent.apply", exact_batch(9410, vec![op])).unwrap();
        let order = if reverse { [second_identity, first] } else { [first, second_identity] };
        for (n, id) in order.iter().enumerate() {
            read(&mut s, 9411 + n);
            s.execute("agent.apply", exact_batch(9411 + n, vec![restore_op(id)])).unwrap();
        }
        let q = s.active_sequence().unwrap();
        assert_eq!(q.video_tracks[0].items.iter().filter(|c| c.start < rate.tick_of(30)).fold(Tick::ZERO, |n, c| n + c.duration), rate.tick_of(20));
        assert_eq!(
            q.video_tracks[0].items.iter().filter(|c| c.start >= rate.tick_of(40) && c.start < rate.tick_of(52)).fold(Tick::ZERO, |n, c| n + c.duration),
            rate.tick_of(12)
        );
        assert_eq!(q.video_tracks[0].items.last().unwrap().start, rate.tick_of(60));
        assert_eq!(
            super::condense_restore::catalog(&s, s.state.active_sequence.unwrap())
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["recordedState"] == "original_native_span")
                .count(),
            2
        );
    }
}

#[test]
fn native_restore_recreates_wholly_removed_partial_audio_with_native_identity_and_original_notes() {
    let mut s = setup();
    let rate = s.sequence_rate();
    let q = s.active_sequence().unwrap();
    let video = q.video_tracks[0].items[0].id.0;
    let audio = q.audio_tracks[0].items[0].id.0;
    let item = q.audio_tracks[0].items[0].item.0;
    s.execute("sequence.addTracks", json!({"video":0,"audio":1})).unwrap();
    let extra = s
        .execute(
            "timeline.place",
            json!({"item":item,"track":"A2","frame":10,"sourceIn":rate.tick_of(200).0,"duration":rate.tick_of(1).0,"video":false,"audio":true,"insert":false}),
        )
        .unwrap()["clips"][0]
        .as_u64()
        .unwrap();
    s.execute("clip.link", json!({"clips":[video,audio,extra]})).unwrap();
    s.execute("poema.annotateClips", json!({"clips":[extra],"annotations":{"editor_note":"Partial recorder must return"}})).unwrap();
    let original = s.active_sequence().unwrap().audio_tracks[1].items[0].clone();
    let identity = s.active_sequence().unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
    let op = operation(&s);
    read(&mut s, 9420);
    s.execute("agent.apply", exact_batch(9420, vec![op])).unwrap();
    assert!(s.active_sequence().unwrap().audio_tracks[1].items.is_empty());
    read(&mut s, 9421);
    s.execute("agent.apply", exact_batch(9421, vec![restore_op(&identity)])).unwrap();
    let c = &s.active_sequence().unwrap().audio_tracks[1].items[0];
    assert_ne!(c.id, original.id);
    assert_eq!(c.identity.as_ref().unwrap().parent.as_deref(), Some(original.identity.as_ref().unwrap().id.as_str()));
    assert_eq!(c.identity.as_ref().unwrap().annotations, original.identity.as_ref().unwrap().annotations);
    assert_eq!(
        (c.start, c.duration, c.source_in, c.audio_stream, &c.source_channels),
        (original.start, original.duration, original.source_in, original.audio_stream, &original.source_channels)
    );
}

#[test]
fn native_restore_survives_serialized_restart_recovery_and_native_redo_without_snapshot_replacement() {
    let (mut s, identity) = condensed();
    let base = s.project.clone();
    let state = s.state.clone();
    let media = s.media.clone();
    let mut reopened = crate::Session::new(s.services.clone());
    reopened.project = Arc::new(serde_json::from_slice(&serde_json::to_vec(&*base).unwrap()).unwrap());
    reopened.state = state.clone();
    reopened.media = media;
    read(&mut reopened, 9430);
    reopened.execute("agent.apply", exact_batch(9430, vec![restore_op(&identity)])).unwrap();
    let restored = reopened.project.clone();
    let recovery = reopened.journal.last().unwrap().clone();
    reopened.execute("edit.undo", json!({})).unwrap();
    assert_eq!(reopened.project, base);
    reopened.execute("edit.redo", json!({})).unwrap();
    assert!(Arc::ptr_eq(&reopened.project, &restored));
    s.execute(&recovery.0, recovery.1).unwrap();
    assert_eq!(s.active_sequence().unwrap().video_tracks, reopened.active_sequence().unwrap().video_tracks);
    assert_eq!(s.active_sequence().unwrap().audio_tracks, reopened.active_sequence().unwrap().audio_tracks);
    read(&mut s, 9431);
    let before = s.project.clone();
    assert!(s.execute("agent.apply", exact_batch(9431, vec![restore_op(&identity)])).is_err());
    assert!(Arc::ptr_eq(&s.project, &before));
}

#[test]
fn native_restore_corrupt_archives_refuse_snapshot_reads_without_panics_or_mutation() {
    for field in ["unknown", "range", "count", "source", "overlap"] {
        let (mut s, _) = condensed();
        let sequence = s.state.active_sequence.unwrap();
        let item = Arc::make_mut(&mut s.project).item_mut(sequence).unwrap();
        let raw = item.metadata.get_mut("poema.condense.restore.v1").unwrap();
        let mut v: Value = serde_json::from_str(raw).unwrap();
        match field {
            "unknown" => v["records"][0]["hidden"] = json!(true),
            "range" => v["records"][0]["gaps"][0]["start"] = json!(i64::MAX),
            "count" => v["records"] = json!(vec![v["records"][0].clone(); 33]),
            "source" => v["records"][0]["members"][0]["original"]["duration"] = json!(-1),
            _ => {
                v["records"][0]["members"][0]["pieces"][0]["duration"] = json!(i64::MAX);
            }
        }
        *raw = serde_json::to_string(&v).unwrap();
        let before = s.project.clone();
        let revision = s.revision;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.execute("agent.snapshot", json!({"token":token(9440)}))));
        assert!(outcome.is_ok());
        assert!(outcome.unwrap().is_err(), "{field}");
        assert!(Arc::ptr_eq(&s.project, &before));
        assert_eq!(s.revision, revision);
    }
}

#[test]
fn native_restore_retains_current_auto_points_and_keyframes_without_joining_differently_finished_fragments() {
    let (mut s, identity) = condensed();
    let frame = s.sequence_rate().frame_duration();
    s.edit_sequence("Human finishing after cut", |q, _, _| {
        let p = q.video_tracks[0].items[0].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("position").unwrap();
        p.value = ParamValue::Vec2(filmcraft_geom::Vec2::new(f64::NAN, f64::NAN));
        p.keyframes.push(filmcraft_project::Keyframe::new(frame, ParamValue::Vec2(filmcraft_geom::Vec2::new(f64::NAN, 31.0))));
        q.video_tracks[0].items[1].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("position").unwrap().value =
            ParamValue::Vec2(filmcraft_geom::Vec2::new(0.0, 0.0));
        Ok(())
    })
    .unwrap();
    read(&mut s, 9450);
    let r = s.execute("agent.apply", exact_batch(9450, vec![restore_op(&identity)])).unwrap();
    assert_eq!(r["results"][0]["canonicalVersionSelected"], false);
    let q = s.active_sequence().unwrap();
    let p = &q.video_tracks[0].items[0].effects.iter().find(|e| e.effect == "motion").unwrap().params["position"];
    assert!(p.value.as_vec2().unwrap().x.is_nan());
    assert!(p.value.as_vec2().unwrap().y.is_nan());
    assert!(p.keyframes[0].value.as_vec2().unwrap().x.is_nan());
    assert_eq!(p.keyframes[0].value.as_vec2().unwrap().y, 31.0);
    assert_eq!(p.keyframes[0].time, frame);
    let p = &q.video_tracks[0].items[1].effects.iter().find(|e| e.effect == "motion").unwrap().params["position"];
    assert_eq!(p.value.as_vec2().unwrap(), filmcraft_geom::Vec2::new(0.0, 0.0));
}

#[test]
fn native_restore_from_real_saved_project_keeps_saved_notes_and_reopens_the_restored_record() {
    let (mut s, identity) = condensed();
    let directory = crate::test_fixture_root(std::env::var_os("FILMCRAFT_FIXTURES_DIR"), std::env::var("POEMA_FIXTURE_ROOT_REQUIRED").as_deref() == Ok("1"))
        .unwrap()
        .join(format!("native-span-restore-{}-{}", std::process::id(), token(9460)));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generated-native-span.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    let mut reopened = crate::Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    read(&mut reopened, 9460);
    reopened.execute("agent.apply", exact_batch(9460, vec![restore_op(&identity)])).unwrap();
    reopened.execute("file.save", json!({})).unwrap();
    let saved = serde_json::to_value(&*reopened.project).unwrap();
    let mut final_copy = crate::Session::default();
    final_copy.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*final_copy.project).unwrap(), saved);
    assert_eq!(super::condense_restore::catalog(&final_copy, final_copy.state.active_sequence.unwrap()).unwrap()[0]["recordedState"], "original_native_span");
}

#[test]
fn native_restore_hostile_batches_and_stale_tickets_do_not_publish_or_consume_human_history() {
    for (field, value) in [("identity", json!(null)), ("receiptSha256", json!("bad")), ("projectId", json!(null)), ("extra", json!(true))] {
        let (mut s, id) = condensed();
        let mut op = restore_op(&id);
        op["params"][field] = value;
        rejected(&mut s, op);
    }
    for case in 0..3 {
        let (mut s, id) = condensed();
        read(&mut s, 9470);
        let mut request = exact_batch(9470, vec![restore_op(&id)]);
        match case {
            0 => request["exact"] = json!(false),
            1 => request["operations"] = json!([restore_op(&id), restore_op(&id)]),
            _ => {
                s.execute("mixer.setStrip", json!({"strip":"A1","pan":20})).unwrap();
            }
        }
        let before = (s.project.clone(), s.revision, s.history.undo.len(), s.history.redo.len(), s.journal.clone());
        assert!(s.execute("agent.apply", request).is_err());
        assert!(Arc::ptr_eq(&s.project, &before.0));
        assert_eq!(s.revision, before.1);
        assert_eq!((s.history.undo.len(), s.history.redo.len()), (before.2, before.3));
        assert_eq!(s.journal, before.4);
    }
}

#[test]
fn native_condense_disjoint_passages_preserve_each_linked_group_and_one_guarded_undo() {
    let mut s = setup();
    let original = s.project.clone();
    let rate = s.sequence_rate();
    let second = s.active_sequence().unwrap().video_tracks[0].items[1].clone();
    let mut op = operation(&s);
    op["params"]["passages"].as_array_mut().unwrap().push(json!({
        "clip":second.id.0,"identity":second.identity.as_ref().unwrap().id,"sourceIn":second.source_in.0,
        "sourceOut":second.source_out().0,"start":second.start.0,
        "kept":[[rate.tick_of(100).0,rate.tick_of(104).0],[rate.tick_of(106).0,rate.tick_of(112).0]]
    }));
    let revision = s.revision;
    read(&mut s, 9200);
    let r = s.execute("agent.apply", exact_batch(9200, vec![op])).unwrap();
    assert_eq!(r["revision"], revision + 1);
    assert_eq!(r["results"][0]["condensed"], 2);
    assert_eq!(r["results"][0]["removedTicks"], rate.tick_of(8).0);
    read(&mut s, 9201);
    s.execute("agent.undo", json!({"token":token(9201),"commit":token(9200)})).unwrap();
    assert!(Arc::ptr_eq(&original, &s.project));
}

#[test]
fn native_condense_keeps_complete_picture_and_jl_sound_with_one_commit_and_guarded_undo_redo() {
    let mut s = setup();
    let original = s.project.clone();
    let rev = s.revision;
    let op = operation(&s);
    read(&mut s, 9000);
    let request = exact_batch(9000, vec![op]);
    let receipt = s.execute("agent.apply", request.clone()).unwrap();
    let identity = original.sequence(s.state.active_sequence.unwrap()).unwrap().video_tracks[0].items[0].identity.as_ref().unwrap().id.clone();
    assert_eq!(
        receipt["results"][0]["passageRanges"],
        json!([{"identity":identity,"startTicks":s.sequence_rate().tick_of(10).0,"endTicks":s.sequence_rate().tick_of(24).0}])
    );
    assert_eq!(receipt["validated"], true);
    assert_eq!(receipt["results"][0]["removals"], 3);
    assert_eq!(receipt["results"][0]["removedTicks"], s.sequence_rate().tick_of(6).0);
    assert_eq!(s.revision, rev + 1);
    assert_eq!(s.history.undo.len(), 1);
    assert_eq!(s.journal.len(), 1);
    let q = s.active_sequence().unwrap();
    let rate = s.sequence_rate();
    let spans = |items: &[filmcraft_project::TrackItem]| {
        items.iter().map(|c| (rate.frame_at(c.start), rate.frame_at(c.duration), rate.frame_at(c.source_in))).collect::<Vec<_>>()
    };
    assert_eq!(spans(&q.video_tracks[0].items), vec![(10, 6, 102), (16, 8, 110), (34, 12, 100), (54, 12, 100)]);
    assert_eq!(spans(&q.audio_tracks[0].items), vec![(8, 2, 200), (10, 6, 204), (16, 8, 212), (24, 2, 222), (34, 12, 100), (54, 12, 100)]);
    for n in 0..2 {
        assert_eq!(q.video_tracks[0].items[n].link, q.audio_tracks[0].items[n + 1].link);
        assert_eq!(q.audio_tracks[0].items[n + 1].source_channels, vec![0]);
        assert_eq!(q.audio_tracks[0].items[n + 1].gain_db, -3.0);
        assert_eq!(q.video_tracks[0].items[n].identity.as_ref().unwrap().annotations["editor_note"], "Keep this idea");
    }
    assert_ne!(q.video_tracks[0].items[0].link, q.video_tracks[0].items[1].link);
    let edited = s.project.clone();
    assert_eq!(s.execute("agent.apply", request).unwrap(), receipt);
    assert!(Arc::ptr_eq(&s.project, &edited));
    assert_eq!(s.history.undo.len(), 1);
    read(&mut s, 9001);
    s.execute("agent.undo", json!({"token":token(9001),"commit":token(9000)})).unwrap();
    assert_eq!(s.project, original);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(s.project, edited);
}

#[test]
fn native_condense_matches_human_extract_geometry_routing_effects_and_links() {
    let mut agent = setup();
    let mut human = crate::Session::new(agent.services.clone());
    human.project = agent.project.clone();
    human.state = agent.state.clone();
    human.media = agent.media.clone();
    let op = operation(&agent);
    read(&mut agent, 9002);
    agent.execute("agent.apply", exact_batch(9002, vec![op])).unwrap();
    let rate = human.sequence_rate();
    for (a, z) in [(28, 30), (18, 20), (10, 12)] {
        human
            .edit_sequence("Set human Extract marks", |q, _, _| {
                q.mark_in = Some(rate.tick_of(a));
                q.mark_out = Some(rate.tick_of(z - 1));
                Ok(())
            })
            .unwrap();
        human.execute("sequence.extract", json!({})).unwrap();
    }
    let normalize = |s: &crate::Session| {
        let mut q = s.active_sequence().unwrap().clone();
        for t in q.all_tracks_mut() {
            for c in &mut t.items {
                c.identity = None;
            }
        }
        q
    };
    assert_eq!(normalize(&agent), normalize(&human));
}

#[test]
fn native_condense_retains_original_outer_jl_edges_and_partial_members_at_a_splice() {
    for (keep_in, keep_out, partial) in [(100, 120, false), (100, 118, false), (102, 120, false), (100, 120, true)] {
        let mut s = setup();
        let rate = s.sequence_rate();
        let seq = s.state.active_sequence.unwrap();
        if partial {
            let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
            let a = &mut q.audio_tracks[0].items[0];
            a.start = rate.tick_of(20);
            a.duration = rate.tick_of(12);
            a.source_in = rate.tick_of(212);
        }
        let mut op = operation(&s);
        op["params"]["passages"][0]["kept"] = json!([[rate.tick_of(keep_in).0, rate.tick_of(108).0], [rate.tick_of(110).0, rate.tick_of(keep_out).0]]);
        read(&mut s, 9006);
        s.execute("agent.apply", exact_batch(9006, vec![op])).unwrap();
        let q = s.active_sequence().unwrap();
        for c in &q.video_tracks[0].items[..2] {
            for a in q.audio_tracks[0].items.iter().filter(|a| a.start < c.end() && a.end() > c.start) {
                assert_eq!(c.link, a.link, "partial/JL members in the same retained region must stay linked");
            }
        }
    }
}

#[test]
fn native_condense_preserves_native_auto_point_sentinels_in_values_and_keyframes() {
    let mut s = setup();
    let seq = s.state.active_sequence.unwrap();
    let frame = s.sequence_rate().frame_duration();
    let q = Arc::make_mut(&mut s.project).sequence_mut(seq).unwrap();
    let c = &mut q.video_tracks[0].items[0];
    let motion = c.effects.iter_mut().find(|e| e.effect == "motion").unwrap();
    let param = motion.params.get_mut("position").unwrap();
    param.value = ParamValue::Vec2(filmcraft_geom::Vec2::new(f64::NAN, f64::NAN));
    param.keyframes.push(filmcraft_project::Keyframe::new(frame, ParamValue::Vec2(filmcraft_geom::Vec2::new(f64::NAN, 50.0))));
    let exact = c.clone();
    assert_ne!(c, &exact, "ordinary equality rejects a legitimate unchanged native auto point");
    assert!(super::condense::same_item(c, &exact));
    let mut changed = exact.clone();
    changed.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("position").unwrap().value =
        ParamValue::Vec2(filmcraft_geom::Vec2::new(0.0, f64::NAN));
    assert!(!super::condense::same_item(c, &changed), "auto must not alias an explicit zero");
    let mut changed = exact;
    changed.effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("position").unwrap().keyframes[0].time += frame;
    assert!(!super::condense::same_item(c, &changed), "matching auto points must not hide changed keyframes");
    let original = s.project.clone();
    let op = operation(&s);
    read(&mut s, 9007);
    s.execute("agent.apply", exact_batch(9007, vec![op])).unwrap();
    for c in &s.active_sequence().unwrap().video_tracks[0].items[..2] {
        let p = &c.effects.iter().find(|e| e.effect == "motion").unwrap().params["position"];
        assert!(p.value.as_vec2().unwrap().x.is_nan());
        assert!(p.value.as_vec2().unwrap().y.is_nan());
        assert!(p.keyframes[0].value.as_vec2().unwrap().x.is_nan());
        assert_eq!(p.keyframes[0].value.as_vec2().unwrap().y, 50.0);
    }
    read(&mut s, 9008);
    s.execute("agent.undo", json!({"token":token(9008),"commit":token(9007)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &original));
}

#[test]
fn native_condense_hostile_shape_and_inexact_or_stale_targets_preserve_project_history_and_redo() {
    for (field, value) in
        [("version", json!(0)), ("receiptSha256", json!("bad")), ("projectId", json!("foreign")), ("passages", json!([])), ("extra", json!(true))]
    {
        let mut s = setup();
        let mut op = operation(&s);
        op["params"][field] = value;
        rejected(&mut s, op);
    }
    for (field, value) in [
        ("identity", json!("a".repeat(32))),
        ("clip", json!(0)),
        ("start", json!(0)),
        ("sourceOut", json!(9007199254740992u64)),
        ("kept", json!([])),
        ("kept", json!([[0, 1]])),
        ("kept", json!([[1.5, 2]])),
    ] {
        let mut s = setup();
        let mut op = operation(&s);
        op["params"]["passages"][0][field] = value;
        rejected(&mut s, op);
    }
    let mut s = setup();
    let mut op = operation(&s);
    op["params"]["passages"][0]["kept"][0][0] = json!(s.sequence_rate().tick_of(102).0 + 1);
    rejected(&mut s, op);
}

#[test]
fn native_condense_refuses_unsafe_collateral_locks_protection_clocks_and_allocation() {
    for mode in ["locked", "protected", "untracked", "caption", "speed", "reverse", "hold", "remap", "remix", "foreign", "collateral", "counter"] {
        let mut s = setup();
        let op = operation(&s);
        let seq = s.state.active_sequence.unwrap();
        let frame = s.sequence_rate().frame_duration();
        if mode == "counter" {
            Arc::make_mut(&mut s.project).next_id = 9_007_199_254_740_991;
        } else {
            if mode == "collateral" {
                s.execute("sequence.addTracks", json!({"video":1,"audio":0})).unwrap();
            }
            let p = Arc::make_mut(&mut s.project);
            let q = p.sequence_mut(seq).unwrap();
            let a = &mut q.audio_tracks[0].items[0];
            match mode {
                "locked" => q.audio_tracks[0].locked = true,
                "protected" => a.identity.as_mut().unwrap().protection = Some("do not change".into()),
                "untracked" => a.identity = None,
                "caption" => {
                    q.caption_tracks.push(filmcraft_project::CaptionTrack::new(filmcraft_project::TrackId(900000), "captions".into(), Default::default()));
                }
                "speed" => a.speed = 2.0,
                "reverse" => a.reverse = true,
                "hold" => a.frame_hold = Some(frame),
                "remap" => {
                    let mut e = filmcraft_project::effect::intrinsic_video().into_iter().find(|e| e.effect == "time_remap").unwrap();
                    e.params.get_mut("speed").unwrap().value = ParamValue::Float(110.0);
                    a.effects.push(e);
                }
                "remix" => {
                    let mut e = filmcraft_project::effect::intrinsic_video().remove(0);
                    e.effect = "remix".into();
                    a.effects.push(e);
                }
                "foreign" => {
                    let item = a.item;
                    p.items
                        .get_mut(&item)
                        .unwrap()
                        .metadata
                        .insert("poema.binding.v1".into(), json!({"projectId":"foreign","assetId":"x","mediaId":"y","role":"original"}).to_string());
                }
                "collateral" => {
                    let mut c = q.video_tracks[0].items.remove(1);
                    c.start = Tick(frame.0 * 18);
                    c.duration = frame;
                    q.video_tracks[1].items.push(c);
                }
                _ => {}
            }
        }
        rejected(&mut s, op);
    }
}

#[test]
fn native_condense_recovery_replays_one_logical_operation_and_serialized_round_trip_retains_fragments() {
    let mut s = setup();
    let original = s.project.clone();
    let state = s.state.clone();
    let media = s.media.clone();
    let op = operation(&s);
    read(&mut s, 9003);
    s.execute("agent.apply", exact_batch(9003, vec![op])).unwrap();
    assert_eq!(s.journal.len(), 1);
    assert_eq!(s.journal[0].0, "agent.replayBatch");
    let (command, params) = s.journal[0].clone();
    let mut recovered = crate::Session::new(s.services.clone());
    recovered.project = original.clone();
    recovered.state = state;
    recovered.media = media;
    recovered.execute(&command, params).unwrap();
    assert_eq!(recovered.history.undo.len(), 1);
    let a = s.active_sequence().unwrap();
    let b = recovered.active_sequence().unwrap();
    assert_eq!(
        a.video_tracks[0].items.iter().map(|c| (c.start, c.duration, c.source_in, c.link)).collect::<Vec<_>>(),
        b.video_tracks[0].items.iter().map(|c| (c.start, c.duration, c.source_in, c.link)).collect::<Vec<_>>()
    );
    let bytes = serde_json::to_vec(&*s.project).unwrap();
    let reopened: filmcraft_project::Project = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(reopened, *s.project);
    filmcraft_project::clip_identity::validate_project(&reopened).unwrap();
    recovered.execute("edit.undo", json!({})).unwrap();
    assert_eq!(recovered.project, original);
}

#[test]
fn native_condense_real_saved_project_reopens_with_exact_native_fragments_and_metadata() {
    let mut s = setup();
    let op = operation(&s);
    read(&mut s, 9004);
    s.execute("agent.apply", exact_batch(9004, vec![op])).unwrap();
    let directory = crate::test_fixture_root(std::env::var_os("FILMCRAFT_FIXTURES_DIR"), std::env::var("POEMA_FIXTURE_ROOT_REQUIRED").as_deref() == Ok("1"))
        .unwrap()
        .join(format!("native-condense-{}-{}", std::process::id(), token(9004)));
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("generated-condense.fcproj");
    s.execute("file.saveAs", json!({"path":file.to_string_lossy()})).unwrap();
    let saved = serde_json::to_value(&*s.project).unwrap();
    let mut reopened = crate::Session::default();
    reopened.execute("file.open", json!({"path":file.to_string_lossy()})).unwrap();
    assert_eq!(serde_json::to_value(&*reopened.project).unwrap(), saved);
}
