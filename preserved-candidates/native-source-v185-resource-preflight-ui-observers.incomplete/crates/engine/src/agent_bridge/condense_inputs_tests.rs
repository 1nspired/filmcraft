use super::condense_tests::{operation, setup};
use super::tests::{exact_batch, read, token};
use filmcraft_project::ParamValue;
use serde_json::{Value, json};
use std::sync::Arc;

fn cut() -> (crate::Session, String) {
    let mut s = setup();
    let op = operation(&s);
    let id = op["params"]["passages"][0]["identity"].as_str().unwrap().to_string();
    read(&mut s, 25100);
    s.execute("agent.apply", exact_batch(25100, vec![op])).unwrap();
    (s, id)
}
fn request(s: &crate::Session, n: usize, ids: &[String]) -> Value {
    json!({"token":token(n),"scope":{"projectId":"p_test","anchorSequence":s.state.active_sequence.unwrap().0},"identities":ids})
}
fn state(s: &crate::Session) -> Value {
    json!({"project":s.project,"revision":s.revision,"saved":s.saved_revision,"editor":s.state,
    "undo":s.history.undo.len(),"redo":s.history.redo.len(),"merge":s.history.merge_key,"journal":s.journal})
}
fn choice(s: &crate::Session, id: &str, old: &str, new: &str) -> Value {
    let rate = s.sequence_rate();
    json!({"command":"agent.chooseCondense","params":{"projectId":"p_test","receiptSha256":new,"version":2,
        "previousReceipts":[{"identity":id,"receiptSha256":old}],"passages":[{"identity":id,"sourceIn":rate.tick_of(100).0,"sourceOut":rate.tick_of(120).0,
        "kept":[[rate.tick_of(101).0,rate.tick_of(119).0]]}]}})
}

#[test]
fn condense_logical_read_returns_original_not_fragment_clock_and_current_laid_duration_without_any_live_edit() {
    let (mut s, id) = cut();
    let rate = s.sequence_rate();
    s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
    s.undo().unwrap();
    s.history.merge_key = Some("existing human gesture".into());
    read(&mut s, 25101);
    let before = state(&s);
    let project = s.project.clone();
    let tickets = s.agent_bridge.tickets.len();
    let r = s.execute("agent.condenseInputs", request(&s, 25101, std::slice::from_ref(&id))).unwrap();
    assert_eq!(r["saved"], true);
    assert_eq!(r["view"], "original_condense_inputs");
    assert_eq!(r["passages"][0]["laidTicks"], rate.tick_of(14).0);
    assert_eq!(r["passages"][0]["previousReceiptSha256"], "c".repeat(64));
    let c = &r["originalSequence"]["video"][0]["items"][0];
    assert_eq!(c["sourceIn"], rate.tick_of(100).0);
    assert_eq!(c["sourceOut"], rate.tick_of(120).0);
    assert_eq!(c["duration"], rate.tick_of(20).0);
    assert_eq!(c["identity"]["id"], id);
    assert_eq!(state(&s), before);
    assert!(Arc::ptr_eq(&s.project, &project));
    assert_eq!(s.agent_bridge.tickets.len(), tickets);
    assert!(s.agent_bridge.candidates.is_empty());
    // The read ticket remains an unused live ticket, not a synthetic edit token.
    s.execute("agent.validateSnapshot", json!({"token":token(25101)})).unwrap();
}

#[test]
fn condense_independent_passage_after_another_saved_cut_is_admitted_but_owned_fragments_are_not() {
    let (mut s, id) = cut();
    let rate = s.sequence_rate();
    let c = s.active_sequence().unwrap().video_tracks[0].items[2].clone();
    let second = c.identity.as_ref().unwrap().id.clone();
    read(&mut s, 25102);
    let before = state(&s);
    let r = s.execute("agent.condenseInputs", request(&s, 25102, std::slice::from_ref(&second))).unwrap();
    assert_eq!(r["saved"], false);
    assert!(r["passages"][0]["previousReceiptSha256"].is_null());
    assert_eq!(state(&s), before);
    let op = json!({"command":"agent.condense","params":{"projectId":"p_test","receiptSha256":"e".repeat(64),"version":1,"passages":[{
        "clip":c.id.0,"identity":second,"sourceIn":c.source_in.0,"sourceOut":c.source_out().0,"start":c.start.0,
        "kept":[[(c.source_in+rate.tick_of(1)).0,(c.source_out()-rate.tick_of(1)).0]]}]}});
    read(&mut s, 25103);
    s.execute("agent.apply", exact_batch(25103, vec![op])).unwrap();
    let catalog = super::condense_restore::catalog(&s, s.state.active_sequence.unwrap()).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 2);
    let fragment = s.active_sequence().unwrap().video_tracks[0].items[1].clone();
    let fragment_id = fragment.identity.as_ref().unwrap().id.clone();
    read(&mut s, 25104);
    let before = state(&s);
    assert!(s.execute("agent.condenseInputs", request(&s, 25104, &[fragment_id])).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 25105);
    let original = s.execute("agent.condenseInputs", request(&s, 25105, &[id])).unwrap();
    assert_eq!(original["passages"][0]["laidTicks"], rate.tick_of(14).0);
}

#[test]
fn condense_fresh_receipt_transition_keeps_current_human_finishing_and_exact_undo_redo_reopen() {
    let (mut s, id) = cut();
    let sid = s.state.active_sequence.unwrap();
    s.edit_sequence("Current human finishing", |q, _, _| {
        q.video_tracks[0].items[0].name = "Current first finish".into();
        q.video_tracks[0].items[1].effects.iter_mut().find(|e| e.effect == "motion").unwrap().params.get_mut("scale").unwrap().value = ParamValue::Float(79.0);
        q.audio_tracks[0].items[1].gain_db = -7.0;
        Ok(())
    })
    .unwrap();
    let before = s.project.clone();
    let old = "c".repeat(64);
    let new = "d".repeat(64);
    read(&mut s, 25106);
    s.execute("agent.apply", exact_batch(25106, vec![choice(&s, &id, &old, &new)])).unwrap();
    let catalog = super::condense_restore::catalog(&s, sid).unwrap();
    assert_eq!(catalog[0]["receiptSha256"], new);
    assert!(s.active_sequence().unwrap().audio_tracks[0].items.iter().any(|c| c.gain_db == -7.0));
    assert!(
        s.active_sequence().unwrap().video_tracks[0]
            .items
            .iter()
            .any(|c| c.effects.iter().find(|e| e.effect == "motion").unwrap().params["scale"].value == ParamValue::Float(79.0))
    );
    let after = s.project.clone();
    read(&mut s, 25107);
    s.execute("agent.undo", json!({"token":token(25107),"commit":token(25106)})).unwrap();
    assert!(Arc::ptr_eq(&s.project, &before));
    s.redo().unwrap();
    assert!(Arc::ptr_eq(&s.project, &after));
    let saved = filmcraft_format::encode(&s.project, false);
    let mut reopened = crate::Session::new(s.services.clone());
    reopened.project = Arc::new(filmcraft_format::decode(&saved).unwrap().project);
    reopened.state = s.state.clone();
    assert_eq!(super::condense_restore::catalog(&reopened, sid).unwrap()[0]["receiptSha256"], new);
    read(&mut reopened, 25108);
    let r = reopened.execute("agent.condenseInputs", request(&reopened, 25108, &[id])).unwrap();
    assert_eq!(r["passages"][0]["previousReceiptSha256"], new);
}

#[test]
fn condense_logical_reads_and_receipt_transitions_reject_foreign_stale_partial_and_changed_input_without_history_loss() {
    for mode in ["foreign", "stale", "replacement", "route", "protected", "unknown", "mixed"] {
        let (mut s, id) = cut();
        let sequence = s.state.active_sequence.unwrap();
        let mut ids = vec![id.clone()];
        if mode == "unknown" {
            ids = vec!["f".repeat(32)];
        }
        if mode == "mixed" {
            ids.push(s.active_sequence().unwrap().video_tracks[0].items[2].identity.as_ref().unwrap().id.clone());
        }
        read(&mut s, 25109);
        let mut r = request(&s, 25109, &ids);
        if mode == "foreign" {
            r["scope"]["projectId"] = json!("another_film");
        }
        if mode == "stale" {
            s.execute("mixer.setStrip", json!({"strip":"A1","pan":-20})).unwrap();
        }
        if mode == "replacement" {
            s.project = Arc::new((*s.project).clone());
        }
        if mode == "route" {
            Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().audio_tracks[0].items[0].source_channels = vec![1];
            read(&mut s, 25109 + 1);
            r["token"] = json!(token(25109 + 1));
        }
        if mode == "protected" {
            Arc::make_mut(&mut s.project).sequence_mut(sequence).unwrap().video_tracks[0].items[0].identity.as_mut().unwrap().protection =
                Some("Human choice".into());
            read(&mut s, 25109 + 1);
            r["token"] = json!(token(25109 + 1));
        }
        let before = state(&s);
        let project = s.project.clone();
        assert!(s.execute("agent.condenseInputs", r).is_err(), "{mode}");
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&s.project, &project));
    }
    for mode in ["foreign", "partial", "identity", "extra"] {
        let (mut s, id) = cut();
        let mut op = choice(&s, &id, &"c".repeat(64), &"d".repeat(64));
        if mode == "foreign" {
            op["params"]["previousReceipts"][0]["receiptSha256"] = json!("f".repeat(64));
        }
        if mode == "partial" {
            op["params"]["previousReceipts"] = json!([]);
        }
        if mode == "identity" {
            op["params"]["previousReceipts"][0]["identity"] = json!("f".repeat(32));
        }
        if mode == "extra" {
            op["params"]["previousReceipts"][0]["sourceIn"] = json!(0);
        }
        s.execute("mixer.setStrip", json!({"strip":"A1","pan":-30})).unwrap();
        s.undo().unwrap();
        read(&mut s, 25111);
        let before = state(&s);
        let project = s.project.clone();
        assert!(s.execute("agent.apply", exact_batch(25111, vec![op])).is_err());
        assert_eq!(state(&s), before);
        assert!(Arc::ptr_eq(&s.project, &project));
    }
}

#[test]
fn condense_multiple_logical_passages_with_different_previous_receipts_transition_atomically_in_either_order() {
    for reverse in [false, true] {
        let (mut s, first) = cut();
        let rate = s.sequence_rate();
        let c = s.active_sequence().unwrap().video_tracks[0].items[2].clone();
        let second = c.identity.as_ref().unwrap().id.clone();
        let second_cut = json!({"command":"agent.condense","params":{"projectId":"p_test","receiptSha256":"e".repeat(64),"version":1,"passages":[{
            "clip":c.id.0,"identity":second,"sourceIn":c.source_in.0,"sourceOut":c.source_out().0,"start":c.start.0,
            "kept":[[(c.source_in+rate.tick_of(1)).0,(c.source_out()-rate.tick_of(1)).0]]}]}});
        read(&mut s, 25200);
        s.execute("agent.apply", exact_batch(25200, vec![second_cut])).unwrap();
        let mut ids = vec![first.clone(), second.clone()];
        if reverse {
            ids.reverse();
        }
        read(&mut s, 25201);
        let before = s.project.clone();
        let inputs = s.execute("agent.condenseInputs", request(&s, 25201, &ids)).unwrap();
        assert_eq!(inputs["passages"].as_array().unwrap().len(), 2);
        assert!(Arc::ptr_eq(&s.project, &before));
        let passages: Vec<_> = ids
            .iter()
            .map(|id| {
                let (a, z) = if *id == first { (rate.tick_of(100), rate.tick_of(120)) } else { (c.source_in, c.source_out()) };
                json!({"identity":id,"sourceIn":a.0,"sourceOut":z.0,"kept":[[(a+rate.tick_of(2)).0,(z-rate.tick_of(2)).0]]})
            })
            .collect();
        let previous: Vec<_> = ids.iter().map(|id| json!({"identity":id,"receiptSha256":if *id == first {"c".repeat(64)}else{"e".repeat(64)}})).collect();
        let op = json!({"command":"agent.chooseCondense","params":{"projectId":"p_test","receiptSha256":"d".repeat(64),"version":2,
            "previousReceipts":previous,"passages":passages}});
        s.execute("agent.apply", exact_batch(25201, vec![op])).unwrap();
        let sid = s.state.active_sequence.unwrap();
        let catalog = super::condense_restore::catalog(&s, sid).unwrap();
        assert!(catalog.as_array().unwrap().iter().all(|r| r["receiptSha256"] == "d".repeat(64)));
        let after = s.project.clone();
        read(&mut s, 25202);
        s.execute("agent.undo", json!({"token":token(25202),"commit":token(25201)})).unwrap();
        assert!(Arc::ptr_eq(&s.project, &before));
        s.redo().unwrap();
        assert!(Arc::ptr_eq(&s.project, &after));
    }
}
