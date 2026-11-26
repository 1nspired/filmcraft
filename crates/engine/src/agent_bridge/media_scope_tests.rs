use super::{
    assembly_tests, media_guard, media_scope,
    tests::{exact_batch, token},
};
use crate::Session;
use filmcraft_project::{BinEntry, ItemId, ItemKind, MediaRef};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (Session, Scratch, ItemId, ItemId) {
    let (mut s, item) = assembly_tests::fixture();
    let dir = Scratch(std::env::temp_dir().join(format!("filmcraft-source-scope-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
    fs::create_dir(&dir.0).unwrap();
    fs::write(dir.0.join("placed.bin"), b"placed source bytes").unwrap();
    fs::write(dir.0.join("unplaced.bin"), b"unplaced source bytes").unwrap();
    let p = Arc::make_mut(&mut s.project);
    let source = p.item_mut(ItemId(item)).unwrap();
    source.as_media_mut().unwrap().media = MediaRef::File { path: dir.0.join("placed.bin").to_string_lossy().into_owned() };
    let mut extra = source.clone();
    extra.id = ItemId(1_500_000);
    extra.name = "Unplaced original".into();
    extra.as_media_mut().unwrap().media = MediaRef::File { path: dir.0.join("unplaced.bin").to_string_lossy().into_owned() };
    extra.metadata.insert(super::BINDING_KEY.into(), json!({"projectId":"p_test","assetId":"a_unplaced","mediaId":"m_unplaced","role":"original"}).to_string());
    let id = extra.id;
    p.items.insert(id, extra);
    p.root.children.push(BinEntry::Item(id));
    p.next_id = 2_000_000;
    (s, dir, ItemId(item), id)
}
fn read(s: &mut Session, n: usize, scope: Value) -> Value {
    s.execute("agent.snapshot", json!({"token":token(n),"mediaScope":scope})).unwrap()
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo.len(),"redo":s.history.redo.len(),"journal":s.journal,"events":s.events.len()})
}
fn split(s: &Session, n: usize) -> Value {
    let c = &s.active_sequence().unwrap().video_tracks[0].items[0];
    exact_batch(n, vec![json!({"command":"timeline.razor","params":{"clip":c.id.0,"time":c.start.0+s.sequence_rate().tick_of(1).0}})])
}
fn insert(s: &Session, n: usize, item: ItemId) -> Value {
    let q = s.active_sequence().unwrap();
    exact_batch(
        n,
        vec![
            json!({"command":"agent.insert","params":{"item":item.0,"videoTrack":q.video_tracks[0].id.0,"audioTrack":q.audio_tracks[0].id.0,"frame":12,"sourceInFrame":1,"durationFrames":3,"audioChannels":[0]}}),
        ],
    )
}
fn row(doc: &Value, id: ItemId) -> &Value {
    doc["sources"].as_array().unwrap().iter().find(|r| r["item"] == id.0).unwrap()
}

#[test]
fn scoped_snapshot_distinguishes_discovery_from_file_proof_without_touching_the_cut() {
    let (mut s, d, a, b) = setup();
    fs::remove_file(d.0.join("unplaced.bin")).unwrap();
    let before = state(&s);
    let doc = read(&mut s, 72000, json!({}));
    assert_eq!(doc["capabilities"]["scopedSourceVerification"], true);
    assert_eq!(doc["mediaScope"], json!({"kind":"dependencies","items":[a.0],"libraryVerified":false}));
    assert_eq!(row(&doc, a)["fileVerification"], "verified");
    assert_eq!(row(&doc, b)["fileVerification"], "unverified");
    assert_eq!(state(&s), before);
    let full = s.execute("agent.snapshot", json!({"token":token(72001)})).unwrap();
    assert_eq!(full["mediaScope"]["kind"], "library");
    assert_eq!(full["mediaScope"]["libraryVerified"], false);
    assert_eq!(row(&full, b)["fileVerification"], "unavailable");
}

#[test]
fn saved_condense_originals_remain_dependencies_after_a_human_removes_the_visible_pieces() {
    let (_unused, dir, _, _) = setup();
    let mut s = super::condense_tests::setup();
    let sequence = s.state.active_sequence.unwrap();
    let item = s.active_sequence().unwrap().video_tracks[0].items[0].item;
    Arc::make_mut(&mut s.project).item_mut(item).unwrap().as_media_mut().unwrap().media =
        MediaRef::File { path: dir.0.join("placed.bin").to_string_lossy().into_owned() };
    read(&mut s, 72100, json!({}));
    let operation = super::condense_tests::operation(&s);
    s.execute("agent.apply", exact_batch(72100, vec![operation])).unwrap();
    s.execute("edit.selectAll", json!({})).unwrap();
    s.execute("edit.clear", json!({})).unwrap();
    assert!(s.active_sequence().unwrap().all_tracks().all(|t| t.items.is_empty()));
    assert_eq!(media_scope::resolve(&s.project, sequence, None, &media_scope::Request::default()).unwrap(), vec![item.0]);
    // These retained inputs still need fresh full-byte qualification. Removing
    // the visible pieces cannot turn saved planning/restoration into metadata proof.
    let doc = read(&mut s, 72101, json!({}));
    assert_eq!(row(&doc, item)["fileVerification"], "verified");
    fs::write(dir.0.join("placed.bin"), b"changed saved original").unwrap();
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(72101)})).is_err());
}

#[test]
fn unrelated_file_changes_do_not_block_local_edits_but_required_changes_still_refuse_atomically() {
    let (mut s, d, _, _) = setup();
    read(&mut s, 72010, json!({}));
    fs::write(d.0.join("unplaced.bin"), b"unrelated replacement").unwrap();
    s.execute("agent.apply", split(&s, 72010)).unwrap();
    read(&mut s, 72011, json!({}));
    let before = state(&s);
    fs::write(d.0.join("placed.bin"), b"placed REPLACEMENT!").unwrap();
    assert!(s.execute("agent.apply", split(&s, 72011)).is_err());
    assert_eq!(state(&s), before);
    assert!(s.execute("agent.validateSnapshot", json!({"token":token(72011)})).is_err());
}

#[test]
fn insert_and_owned_redo_require_the_actual_new_source_and_never_borrow_discovery_metadata() {
    let (mut s, _d, _, b) = setup();
    read(&mut s, 72020, json!({}));
    let before = state(&s);
    let error = s.execute("agent.apply", insert(&s, 72020, b)).unwrap_err().to_string();
    assert!(error.contains("outside this verified source scope"), "{error}");
    assert_eq!(state(&s), before);
    read(&mut s, 72021, json!({"sources":[b.0]}));
    let inserted = s.execute("agent.apply", insert(&s, 72021, b)).unwrap();
    assert_eq!(inserted["validated"], true);
    read(&mut s, 72022, json!({}));
    s.execute("agent.undo", json!({"token":token(72022),"commit":token(72021)})).unwrap();
    read(&mut s, 72023, json!({}));
    let before = state(&s);
    assert!(s.execute("agent.redo", json!({"token":token(72023),"commit":token(72021),"undo":token(72022)})).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 72024, json!({"assets":[{"projectId":"p_test","assetId":"a_unplaced"}]}));
    s.execute("agent.redo", json!({"token":token(72024),"commit":token(72021),"undo":token(72022)})).unwrap();
}

#[test]
fn source_monitor_marks_require_scoped_bytes_and_future_reads_include_the_open_source() {
    let (mut s, _d, _, b) = setup();
    let preview =
        |n| exact_batch(n, vec![json!({"command":"agent.previewSource","params":{"item":b.0,"projectId":"p_test","sourceInFrame":1,"durationFrames":3}})]);
    read(&mut s, 72030, json!({}));
    let before = state(&s);
    assert!(s.execute("agent.apply", preview(72030)).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 72031, json!({"sources":[b.0]}));
    s.execute("agent.apply", preview(72031)).unwrap();
    let doc = read(&mut s, 72032, json!({}));
    assert_eq!(row(&doc, b)["fileVerification"], "verified");
    assert_eq!(doc["sourceMonitor"]["item"], b.0);
}

#[test]
fn scoped_publication_rechecks_complete_required_file_witnesses_after_staging() {
    let (mut s, d, _, _) = setup();
    read(&mut s, 72040, json!({}));
    let request = split(&s, 72040);
    let batch = super::parse("agent.apply", &request).unwrap();
    let stage = super::stage_batch(&s, &batch).unwrap();
    let before = state(&s);
    fs::write(d.0.join("placed.bin"), b"late replacement").unwrap();
    assert!(super::publish_batch(&mut s, &batch, stage, &request, None).is_err());
    assert_eq!(state(&s), before);
}

fn child(s: &mut Session) -> ItemId {
    let root = s.state.active_sequence.unwrap();
    s.execute("file.newSequence", json!({"name":"Scoped empty sibling","video":1,"audio":1})).unwrap();
    let child = s.state.active_sequence.unwrap();
    Arc::make_mut(&mut s.project)
        .item_mut(child)
        .unwrap()
        .metadata
        .insert(super::sequences::KEY.into(), json!({"projectId":"p_test","anchorSequence":root.0}).to_string());
    child
}

#[test]
fn inactive_sequence_reads_and_navigation_require_their_own_dependencies() {
    let (mut s, _d, a, _) = setup();
    let root = s.state.active_sequence.unwrap();
    child(&mut s);
    let scope = json!({"projectId":"p_test","anchorSequence":root.0});
    let doc = read(&mut s, 72050, json!({}));
    assert_eq!(row(&doc, a)["fileVerification"], "unverified");
    let before = state(&s);
    assert!(s.execute("agent.readCaptionDestination", json!({"token":token(72050),"scope":scope,"sequence":root.0})).is_err());
    assert!(s.execute("agent.selectSequence", json!({"token":token(72050),"scope":scope,"item":root.0})).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 72051, json!({"sequences":[root.0]}));
    s.execute("agent.readCaptionDestination", json!({"token":token(72051),"scope":scope,"sequence":root.0})).unwrap();
    s.execute("agent.selectSequence", json!({"token":token(72051),"scope":scope,"item":root.0})).unwrap();
    assert_eq!(s.state.active_sequence, Some(root));
}

#[test]
fn inactive_caption_publication_and_undo_cannot_borrow_another_reads_expanded_witness() {
    use sha2::{Digest, Sha256};
    let (mut s, _d, _, _) = setup();
    let root = s.state.active_sequence.unwrap();
    child(&mut s);
    let scope = json!({"projectId":"p_test","anchorSequence":root.0});
    read(&mut s, 72060, json!({"sequences":[root.0]}));
    let basis = s.execute("agent.readCaptionDestination", json!({"token":token(72060),"scope":scope,"sequence":root.0})).unwrap();
    let source = "preserved original companion";
    let request = |n| {
        exact_batch(
            n,
            vec![
                json!({"command":"agent.importCaptions","params":{"scope":scope,"beforeLayoutSha256":basis["layoutSha256"],"beforeCaptionSha256":basis["captionSha256"],"beforeArchiveSha256":basis["archiveSha256"],"sourceSha256":format!("{:x}",Sha256::digest(source.as_bytes())),"data":{"sequence":root.0,"sourceDocument":source,"tracks":[{"name":"Original captions","language":"und","format":"Subtitle","cues":[{"start":1,"end":100,"text":"Exact words"}]}]}}}),
            ],
        )
    };
    read(&mut s, 72061, json!({}));
    let before = state(&s);
    assert!(s.execute("agent.apply", request(72061)).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 72062, json!({"sequences":[root.0]}));
    s.execute("agent.apply", request(72062)).unwrap();
    read(&mut s, 72063, json!({}));
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(72063),"commit":token(72062)})).is_err());
    assert_eq!(state(&s), before);
    read(&mut s, 72064, json!({"sequences":[root.0]}));
    s.execute("agent.undo", json!({"token":token(72064),"commit":token(72062)})).unwrap();
}

#[test]
fn scope_resolves_subclips_and_nested_disabled_media_and_rejects_cycles() {
    let (mut s, _d, a, b) = setup();
    let root = s.state.active_sequence.unwrap();
    let settings = s.active_sequence().unwrap().settings.clone();
    let clip = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    let p = Arc::make_mut(&mut s.project);
    let nested = p.new_sequence("Nested scope", settings, 1, 1, None);
    p.sequence_mut(nested).unwrap().video_tracks[0].items.push(clip);
    let mut sub = p.item(a).unwrap().clone();
    sub.id = ItemId(p.alloc_id());
    sub.kind =
        ItemKind::Subclip { parent: b, range: filmcraft_time::TimeRange::new(filmcraft_time::Tick::ZERO, filmcraft_time::Tick(1)), restrict_trims: true };
    let sub_id = sub.id;
    p.items.insert(sub.id, sub);
    let q = p.sequence_mut(root).unwrap();
    q.video_tracks[0].enabled = false;
    q.video_tracks[0].items[0].item = nested;
    q.video_tracks[0].items[1].item = sub_id;
    let request = serde_json::from_value(json!({})).unwrap();
    assert_eq!(media_scope::resolve(p, root, None, &request).unwrap(), vec![a.0, b.0]);
    p.sequence_mut(nested).unwrap().video_tracks[0].items[0].item = root;
    assert!(media_scope::resolve(p, root, None, &request).is_err());
}

#[test]
fn malformed_ambiguous_unbound_and_oversized_scopes_leave_no_ticket_or_history_change() {
    let (mut s, _d, a, b) = setup();
    for scope in [
        Value::Null,
        json!({"sources":[b.0,b.0]}),
        json!({"sequences":[b.0]}),
        json!({"sources":[9_007_199_254_740_992u64]}),
        json!({"sources":[777_777]}),
        json!({"assets":[{"projectId":"p_test","assetId":"missing"}]}),
        json!({"path":"/not-authorized"}),
        json!({"sources":vec![a.0;2049]}),
    ] {
        let before = state(&s);
        let tickets = s.agent_bridge.tickets.len();
        assert!(s.execute("agent.snapshot", json!({"token":token(72070),"mediaScope":scope})).is_err());
        assert_eq!(state(&s), before);
        assert_eq!(s.agent_bridge.tickets.len(), tickets);
    }
    let p = Arc::make_mut(&mut s.project);
    let binding = p.item(a).unwrap().metadata[super::BINDING_KEY].clone();
    p.item_mut(b).unwrap().metadata.insert(super::BINDING_KEY.into(), binding);
    assert!(s.execute("agent.snapshot", json!({"token":token(72071),"mediaScope":{"assets":[{"projectId":"p_test","assetId":"a_source"}]}})).is_err());
    Arc::make_mut(&mut s.project).item_mut(a).unwrap().metadata.remove(super::BINDING_KEY);
    assert!(s.execute("agent.snapshot", json!({"token":token(72072),"mediaScope":{}})).is_err());
}

#[test]
fn complete_witness_union_preserves_each_old_generation_and_never_renews_changed_inputs() {
    let (s, d, a, b) = setup();
    let one = media_guard::Plan::scoped(&s.project, &s.media, vec![a.0]).unwrap().capture(&media_guard::Check::new(web_time::Instant::now())).unwrap();
    let two = media_guard::Plan::scoped(&s.project, &s.media, vec![b.0]).unwrap().capture(&media_guard::Check::new(web_time::Instant::now())).unwrap();
    let plan = media_guard::Plan::for_witnesses(&s.project, &s.media, &[one.clone(), two.clone()]).unwrap();
    assert_eq!(plan.total(), 4);
    let union = plan.capture(&media_guard::Check::new(web_time::Instant::now())).unwrap();
    media_guard::compare(&one, &union).unwrap();
    media_guard::compare(&two, &union).unwrap();
    assert!(media_guard::compare(&union, &one).is_err());
    fs::write(d.0.join("placed.bin"), b"changed generation").unwrap();
    let changed = plan.capture(&media_guard::Check::new(web_time::Instant::now())).unwrap();
    assert!(media_guard::compare(&one, &changed).is_err());
    media_guard::compare(&two, &changed).unwrap();
}

#[test]
fn saved_history_uses_bounded_semantic_audits_instead_of_arc_identity_or_nan_equality() {
    let (mut s, _d, _, b) = setup();
    let root = s.state.active_sequence.unwrap();
    let mut clone = s.active_sequence().unwrap().clone();
    for t in clone.all_tracks_mut() {
        for c in &mut t.items {
            c.item = b;
        }
    }
    let settings = clone.settings.clone();
    let p = Arc::make_mut(&mut s.project);
    for t in clone.all_tracks_mut() {
        t.id = filmcraft_project::TrackId(p.alloc_id());
        for c in &mut t.items {
            c.id = filmcraft_project::ClipId(p.alloc_id());
            c.identity = None;
            c.link = None;
            c.group = None;
        }
    }
    let extra = p.new_sequence("Unchanged saved sibling", settings, 0, 0, None);
    *p.sequence_mut(extra).unwrap() = clone;
    read(&mut s, 72080, json!({}));
    let original = s.project.clone();
    let witnesses = s.agent_bridge.tickets.back().unwrap().media.clone();
    s.execute("agent.apply", split(&s, 72080)).unwrap();
    let before = filmcraft_format::decode(&filmcraft_format::encode(&original, false)).unwrap().project;
    let after = filmcraft_format::decode(&filmcraft_format::encode(&s.project, false)).unwrap().project;
    media_scope::require_transition(
        &before,
        &after,
        &witnesses,
        media_scope::Transition { before_active: Some(root), after_active: Some(root), before_source: None, after_source: None, admitted: Default::default() },
    )
    .unwrap();
}

#[test]
fn scoped_background_reads_keep_their_original_scope_and_cancel_without_authority() {
    let _serial = super::snapshot_jobs::TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut s, _d, a, b) = setup();
    let n = 72090;
    let start = s.execute("agent.snapshot", json!({"token":token(n),"background":true,"mediaScope":{}})).unwrap();
    assert_eq!(start["progress"]["total_checks"], 2);
    assert!(s.execute("agent.snapshot", json!({"token":token(n),"background":true,"mediaScope":{"sources":[b.0]}})).is_err());
    let mut result = Value::Null;
    for _ in 0..1000 {
        let reply = s.execute("agent.snapshotStatus", json!({"job":token(n)})).unwrap();
        if reply["status"] == "complete" {
            result = reply["result"].clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(result["mediaScope"]["items"], json!([a.0]));
    let before = state(&s);
    s.execute("agent.cancelSnapshot", json!({"job":token(n)})).unwrap();
    assert!(s.execute("agent.apply", split(&s, n)).is_err());
    assert_eq!(state(&s), before);
}
