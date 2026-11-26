use super::*;
use crate::{History, agent_bridge::tests::token};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
const W: &str = "0885cc24-f66b-4af1-821c-27110fe518a9";
const C: &str = "ba0bca4f-a2ef-4bcb-ad6f-71e7a9ae3d98";
fn scope() -> Value {
    json!({"projectId":"p_settings_write","workspaceId":W})
}
fn setup(sequence: bool) -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Generated guarded settings","workspace":scope()})).unwrap();
    if sequence {
        s.execute("file.newSequence", json!({"name":"Owning sequence","fps":25,"video":1,"audio":1})).unwrap();
    }
    s.path = Some(crate::temp_dir().join("generated-guarded-settings.fcproj").to_string_lossy().into_owned());
    s.agent_operation_control().enable_transport();
    s
}
fn read(s: &mut Session, n: usize) -> Value {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap()
}
fn request(s: &mut Session, n: usize, setting: Value) -> Value {
    let d = read(s, n);
    assert_eq!(d["capabilities"]["editorSettingsWrite"], true);
    json!({"token":token(n),"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":setting})
}
fn mode(s: &Session, label: &str) -> Value {
    json!({"kind":"clip_mixer_mode","track":s.active_sequence().unwrap().audio_tracks[0].id.0,"mode":label})
}
fn default(definition: &str) -> Value {
    json!({"kind":"default_transition","definition":definition})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"clipboard":s.state.clipboard,"markers":s.state.clipboard_markers,
        "revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"merge":s.history.merge_key,"journal":s.journal})
}
fn history_eq(a: &History, b: &History) {
    assert_eq!(a.limit, b.limit);
    assert_eq!(a.merge_key, b.merge_key);
    for (a, b) in [(&a.undo, &b.undo), (&a.redo, &b.redo)] {
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b).all(|((a, p), (b, q))| a == b && Arc::ptr_eq(p, q)));
    }
}
fn validate(s: &Session, p: &Value, r: &Value) -> Result<()> {
    validate_history(
        &s.project,
        s.path.as_ref().unwrap(),
        &serde_json::from_value(scope()).unwrap(),
        p["token"].as_str().unwrap(),
        r["revision"].as_u64().unwrap(),
        r["sequence"].as_u64(),
        r["sequence"].as_u64(),
        p,
        r,
    )
}
fn paired(s: &mut Session, n: usize, setting: Value) -> (Value, Value) {
    let p = request(s, n, setting);
    let project = s.project.clone();
    let history = s.history.clone();
    let head = s.agent_bridge.head.clone();
    let rev = s.revision;
    let saved = s.saved_revision;
    let mut human = Session::default();
    human.project = s.project.clone();
    human.state = s.state.clone();
    human.path = s.path.clone();
    human.revision = rev;
    human.saved_revision = saved;
    human.history = history.clone();
    human.journal = s.journal.clone();
    let typed = request_from(&p);
    let (command, args) = super::human(&typed.setting);
    human.execute(command, args).unwrap();
    let r = s.execute(COMMAND, p.clone()).unwrap();
    assert_eq!(state(s), state(&human));
    assert!(Arc::ptr_eq(&s.project, &project));
    history_eq(&s.history, &history);
    assert_eq!((s.revision, s.saved_revision), (rev, saved));
    assert_eq!(s.agent_bridge.head.as_ref().map(|(n, _)| *n), head.as_ref().map(|(n, _)| *n));
    if let (Some((_, a)), Some((_, b))) = (&s.agent_bridge.head, &head) {
        assert!(a.ptr_eq(b));
    }
    assert_eq!(r["status"], "configured");
    assert_eq!(r["projectUnchanged"], true);
    assert_eq!(r["historyUnchanged"], true);
    assert_eq!(r["mediaEvidence"], false);
    assert_eq!(r["writeAuthority"], false);
    assert_eq!(r["sourceValidation"], "not-performed");
    validate(s, &p, &r).unwrap();
    let after = state(s);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(state(s), after);
    (p, r)
}
fn request_from(v: &Value) -> Request {
    super::request(v).unwrap()
}
#[test]
fn guarded_settings_all_five_modes_match_original_human_journal_history_and_explicit_read_storage() {
    for (i, m) in AutomationMode::ALL.into_iter().enumerate() {
        let mut s = setup(true);
        s.execute("file.newBin", json!({"name":"Human redo"})).unwrap();
        s.undo().unwrap();
        s.history.merge_key = Some("human fader".into());
        let id = s.state.active_sequence.unwrap();
        Arc::make_mut(&mut s.project).sequence_mut(id).unwrap().audio_tracks[0].locked = true;
        let setting = mode(&s, m.label());
        let (_, r) = paired(&mut s, 160000 + i * 2, setting.clone());
        assert_eq!(r["changed"], true);
        let (_, again) = paired(&mut s, 160001 + i * 2, setting);
        assert_eq!(again["changed"], false);
    }
}
#[test]
fn guarded_settings_every_actual_transition_matches_original_defaults_and_journal_without_sequence_anchor() {
    let defs =
        filmcraft_project::effect_defs().iter().filter(|d| matches!(d.kind, EffectKind::AudioTransition | EffectKind::VideoTransition)).collect::<Vec<_>>();
    assert_eq!(defs.iter().filter(|d| d.kind == EffectKind::AudioTransition).count(), 3);
    assert_eq!(defs.iter().filter(|d| d.kind == EffectKind::VideoTransition).count(), 112);
    for (i, d) in defs.iter().enumerate() {
        let mut s = setup(false);
        let (_, r) = paired(&mut s, 160100 + i, default(d.id));
        assert_eq!(r["sequence"], Value::Null);
        assert!(s.project.sequences().next().is_none());
        let before = s.journal.len();
        let (_, r) = paired(&mut s, 160300 + i, default(d.id));
        assert_eq!(r["changed"], false);
        assert_eq!(s.journal.len(), before + 1);
    }
}
#[test]
fn guarded_settings_catalog_pages_the_original_canonical_transition_registry_without_mutation_or_source_authority() {
    let mut s = setup(false);
    let d = read(&mut s, 160500);
    assert_eq!(d["capabilities"]["editorTransitionCatalog"], true);
    let before = state(&s);
    let mut offset = 0;
    let mut rows = Vec::new();
    loop {
        let r =
            s.execute(CATALOG, json!({"token":token(160500),"scope":scope(),"beforeEditorSha256":d["editorStateSha256"],"offset":offset,"limit":7})).unwrap();
        assert_eq!(r["total"], 115);
        assert_eq!(r["writeAuthority"], false);
        assert_eq!(r["mediaEvidence"], false);
        rows.extend(r["definitions"].as_array().unwrap().clone());
        let Some(next) = r["nextOffset"].as_u64() else { break };
        offset = next;
    }
    let defs =
        filmcraft_project::effect_defs().iter().filter(|d| matches!(d.kind, EffectKind::AudioTransition | EffectKind::VideoTransition)).collect::<Vec<_>>();
    assert_eq!(rows.len(), defs.len());
    for (a, b) in rows.iter().zip(defs) {
        assert_eq!(a["definition"], b.id);
        assert_eq!(a["name"], b.name);
        assert_eq!(a["defaultEligible"], true);
    }
    for (offset, limit) in [(0, 0), (0, 33), (116, 1)] {
        assert!(
            s.execute(CATALOG, json!({"token":token(160500),"scope":scope(),"beforeEditorSha256":d["editorStateSha256"],"offset":offset,"limit":limit}))
                .is_err()
        );
    }
    assert_eq!(state(&s), before);
}
#[test]
fn guarded_settings_hostile_payloads_aliases_wrong_kinds_targets_and_claimed_authority_refuse_before_publication() {
    for (i, kind) in [
        "zero",
        "unsafe-id",
        "video",
        "unknown-mode",
        "extra-setting",
        "unknown-kind",
        "alias",
        "wrong-effect",
        "empty-definition",
        "extra-request",
        "wrong-root",
        "bad-hash",
        "wrong-project",
        "oversize-definition",
    ]
    .into_iter()
    .enumerate()
    {
        let mut s = setup(true);
        let setting = mode(&s, "Touch");
        let mut p = request(&mut s, 160600 + i, setting);
        match kind {
            "zero" => p["setting"]["track"] = json!(0),
            "unsafe-id" => p["setting"]["track"] = json!(9007199254740992u64),
            "video" => p["setting"]["track"] = json!(s.active_sequence().unwrap().video_tracks[0].id.0),
            "unknown-mode" => p["setting"]["mode"] = json!("touch"),
            "extra-setting" => p["setting"]["allowClamp"] = json!(true),
            "unknown-kind" => p["setting"]["kind"] = json!("command"),
            "alias" => p["setting"] = default("Constant Gain"),
            "wrong-effect" => p["setting"] = default("volume"),
            "empty-definition" => p["setting"] = default(""),
            "extra-request" => p["writeAuthority"] = json!(true),
            "wrong-root" => p["scope"]["workspaceId"] = json!(C),
            "bad-hash" => p["beforeEditorSha256"] = json!("A".repeat(64)),
            "wrong-project" => p["beforeSha256"] = json!("0".repeat(64)),
            _ => p["setting"] = default(&"a".repeat(257)),
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "accepted {kind}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn guarded_settings_stale_clipboard_foreign_sequence_library_and_ambiguous_inventory_cannot_borrow_root_purpose() {
    for (i, kind) in ["editor", "clipboard", "project", "library", "sequence", "foreign-active", "duplicate"].into_iter().enumerate() {
        let mut s = setup(true);
        if kind == "foreign-active" {
            s.execute("file.newSequence", json!({"name":"Unowned non-anchor view","fps":25,"video":1,"audio":1})).unwrap();
            let id = s.state.active_sequence.unwrap();
            Arc::make_mut(&mut s.project).item_mut(id).unwrap().metadata.remove(crate::editorial_workspace::SEQUENCE_KEY);
        }
        if kind == "duplicate" {
            let id = s.state.active_sequence.unwrap();
            let q = Arc::make_mut(&mut s.project).sequence_mut(id).unwrap();
            q.audio_tracks.push(q.audio_tracks[0].clone());
        }
        let setting = mode(&s, "Latch");
        let mut p = request(&mut s, 160700 + i, setting);
        match kind {
            "editor" => s.state.source_playhead = filmcraft_time::Tick(1),
            "clipboard" => s
                .state
                .clipboard_markers
                .push(serde_json::from_value(json!({"id":1,"start":0,"duration":0,"name":"Private","comment":"","color":"Iris","kind":"Comment"})).unwrap()),
            "project" => {
                s.execute("file.newBin", json!({"name":"Later human"})).unwrap();
            }
            "library" => s.agent_bridge.tickets.back_mut().unwrap().source_library = Some(vec![1]),
            "sequence" => {
                s.execute("agent.snapshot", json!({"token":token(160790)})).unwrap();
                p["token"] = json!(token(160790));
            }
            _ => {}
        }
        let before = state(&s);
        assert!(s.execute(COMMAND, p).is_err(), "accepted {kind}");
        assert_eq!(state(&s), before);
    }
}
#[test]
fn guarded_settings_preserve_actual_preview_and_recording_owners_and_refuse_hidden_trim_settlement() {
    for (i, recording) in [false, true].into_iter().enumerate() {
        for (j, transition) in [false, true].into_iter().enumerate() {
            let mut s = setup(true);
            let track = s.active_sequence().unwrap().audio_tracks[0].id;
            let setting = if transition { default("constant_gain") } else { mode(&s, "Touch") };
            let p = request(&mut s, 160800 + i * 4 + j, setting);
            if recording {
                s.execute("mixer.recordStart", json!({"time":0})).unwrap();
                assert!(s.mixrec.active());
            } else {
                s.execute("clipMixer.touch", json!({"track":track.0,"lane":"volume","value":-9,"time":0})).unwrap();
                assert!(s.previews.live.is_active());
            }
            let project = s.project.clone();
            let history = s.history.clone();
            let rev = s.revision;
            s.execute(COMMAND, p).unwrap();
            assert!(Arc::ptr_eq(&project, &s.project));
            history_eq(&history, &s.history);
            assert_eq!(s.revision, rev);
            if recording {
                assert!(s.mixrec.active());
            } else {
                assert!(s.previews.live.is_active());
            }
        }
    }
    let mut s = setup(true);
    s.set_playhead(filmcraft_time::Tick::ZERO);
    let p = request(&mut s, 160820, default("constant_gain"));
    s.execute("trim.playAround", json!({"clock":0})).unwrap();
    assert!(s.trim_play.active());
    let before = state(&s);
    let e = s.execute(COMMAND, p).unwrap_err();
    assert!(e.to_string().contains("finish or cancel active trim"), "{e}");
    assert_eq!(state(&s), before);
    assert!(s.trim_play.active());
}
#[test]
fn guarded_settings_stop_lost_reply_and_later_human_settings_reconcile_receipts_without_replay_or_undo_ownership() {
    let mut s = setup(true);
    let setting = mode(&s, "Touch");
    let p = request(&mut s, 160900, setting.clone());
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(160900)})).unwrap()["cancellationAccepted"], true);
    let before = state(&s);
    assert!(s.execute(COMMAND, p).is_err());
    assert_eq!(state(&s), before);
    let p = request(&mut s, 160901, setting);
    let r = s.execute(COMMAND, p.clone()).unwrap();
    let track = s.active_sequence().unwrap().audio_tracks[0].id;
    s.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Write"})).unwrap();
    let later = state(&s);
    assert_eq!(s.execute("agent.receipt", json!({"token":token(160901)})).unwrap()["result"], r);
    assert_eq!(s.execute(COMMAND, p.clone()).unwrap(), r);
    assert_eq!(state(&s), later);
    let mut changed = p;
    changed["setting"]["mode"] = json!("Latch");
    assert!(s.execute(COMMAND, changed).is_err());
    assert_eq!(state(&s), later);
    assert_eq!(s.agent_operation_control().cancel(&json!({"token":token(160901)})).unwrap()["cancellationAccepted"], false);
    read(&mut s, 160902);
    let before = state(&s);
    assert!(s.execute("agent.undo", json!({"token":token(160902),"commit":token(160901)})).is_err());
    assert_eq!(state(&s), before);
    let mut no_transport = setup(false);
    no_transport.agent_bridge.operations = super::super::operation_control::AgentOperationControl::default();
    let d = read(&mut no_transport, 160903);
    assert_eq!(d["capabilities"]["editorSettingsWrite"], false);
    let p = json!({"token":token(160903),"scope":scope(),"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"],"setting":default("constant_gain")});
    let before = state(&no_transport);
    assert!(no_transport.execute(COMMAND, p).is_err());
    assert_eq!(state(&no_transport), before);
}
fn rehash(proof: &mut Value) {
    let data = proof["data"].as_str().unwrap();
    let bytes = data.len();
    let sha = format!("{:x}", Sha256::digest(data.as_bytes()));
    proof["bytes"] = json!(bytes);
    proof["sha256"] = json!(sha);
}
#[test]
fn guarded_settings_opaque_proofs_keep_exact_native_ticks_and_reject_rehashed_unrelated_editor_or_authority_changes() {
    let mut s = setup(true);
    s.state.source_playhead = filmcraft_time::Tick(9007199254740993);
    let setting = mode(&s, "Touch");
    let p = request(&mut s, 161000, setting);
    let r = s.execute(COMMAND, p.clone()).unwrap();
    assert!(r["editorProof"]["data"].as_str().unwrap().contains("9007199254740993"));
    for kind in ["clock", "after", "unknown", "clipboard", "authority"] {
        let mut bad = r.clone();
        if kind == "authority" {
            bad["writeAuthority"] = json!(true);
        } else {
            let mut inner: Value = serde_json::from_str(bad["editorProof"]["data"].as_str().unwrap()).unwrap();
            match kind {
                "clock" => inner["beforeEditor"]["source_playhead"] = json!(9007199254740992i64),
                "after" => inner["afterEditor"]["snapping"] = json!(!s.state.snapping),
                "unknown" => inner["afterEditor"]["unexpected"] = json!(true),
                _ => inner["clipboardSha256"] = json!("0".repeat(64)),
            }
            bad["editorProof"]["data"] = json!(serde_json::to_string(&inner).unwrap());
            rehash(&mut bad["editorProof"]);
        }
        assert!(validate(&s, &p, &bad).is_err(), "accepted {kind}");
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let p = crate::temp_dir().join(format!("filmcraft-editor-setting-history-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();
        Self(std::fs::canonicalize(p).unwrap())
    }
    fn companion(&self) -> PathBuf {
        self.0.join(format!(".poema-history-{W}-{C}.json"))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn persist(s: &mut Session) {
    filmcraft_format::atomic_write(
        std::path::Path::new(s.path.as_ref().unwrap()),
        &filmcraft_format::encode_with_view(&s.project, Some(&s.project_view()), false),
    )
    .unwrap();
    s.saved_revision = s.revision;
}
fn reopened(s: &Session) -> Session {
    let mut cold = Session::default();
    cold.execute("file.open", json!({"path":s.path})).unwrap();
    cold.agent_operation_control().enable_transport();
    cold
}
fn save(s: &mut Session, n: usize) -> Value {
    persist(s);
    read(s, n);
    s.execute("agent.saveHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C})).unwrap()
}
fn restore(s: &mut Session, n: usize, saved: &Value) -> Result<Value> {
    read(s, n);
    s.execute("agent.restoreHistoryCheckpoint", json!({"token":token(n),"scope":scope(),"workspace":W,"checkpoint":C,"sha256":saved["checkpoint"]["sha256"]}))
}
fn owned_bin(s: &mut Session, n: usize) {
    let d = read(s, n);
    s.execute("agent.apply",json!({"token":token(n),"label":"Generated owned bin","exact":true,"authority":"document_state","operations":[{"command":"agent.organizeProject","params":{"scope":scope(),"beforeSha256":d["binState"]["sha256"],"edit":{"type":"create","name":"Owned bin","parent":null}}}]})).unwrap();
}
fn select(s: &mut Session, n: usize) -> Value {
    let d = read(s, n);
    let a = crate::editorial_workspace::association(&s.project).unwrap().unwrap();
    s.execute(super::super::project_navigation::COMMAND,json!({"token":token(n),"scope":scope(),"sequenceScope":a.scope,"item":s.state.active_sequence.unwrap().0,"beforeSha256":d["projectState"]["sha256"],"beforeEditorSha256":d["editorStateSha256"]})).unwrap()
}
#[test]
fn guarded_settings_checkpoint_five_retains_settings_navigation_and_prior_owned_undo_without_replaying_editor_state() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.0.join("generated.fcproj").to_string_lossy().into_owned());
    s.execute("file.newBin", json!({"name":"Prior human work"})).unwrap();
    let before = s.project.clone();
    owned_bin(&mut s, 161100);
    let after = s.project.clone();
    let navigation = select(&mut s, 161101);
    let setting = mode(&s, "Touch");
    let p = request(&mut s, 161102, setting);
    let receipt = s.execute(COMMAND, p.clone()).unwrap();
    let saved = save(&mut s, 161103);
    let raw: Value = serde_json::from_slice(&std::fs::read(dir.companion()).unwrap()).unwrap();
    assert_eq!(raw["version"], 5);
    assert_eq!(raw["tickets"].as_array().unwrap().len(), 3);
    let mut cold = reopened(&s);
    let initial = project_clear::editor_hash(&cold.state).unwrap();
    restore(&mut cold, 161104, &saved).unwrap();
    assert_eq!(project_clear::editor_hash(&cold.state).unwrap(), initial);
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(161101)})).unwrap()["result"], navigation);
    assert_eq!(cold.execute("agent.receipt", json!({"token":token(161102)})).unwrap()["result"], receipt);
    let track = cold.active_sequence().unwrap().audio_tracks[0].id;
    cold.execute("clipMixer.setMode", json!({"track":track.0,"mode":"Write"})).unwrap();
    let later = state(&cold);
    assert_eq!(cold.execute(COMMAND, p).unwrap(), receipt);
    assert_eq!(state(&cold), later);
    read(&mut cold, 161105);
    cold.execute("agent.undo", json!({"token":token(161105),"commit":token(161100)})).unwrap();
    assert_eq!(cold.project, before);
    read(&mut cold, 161106);
    cold.execute("agent.redo", json!({"token":token(161106),"commit":token(161100),"undo":token(161105)})).unwrap();
    assert_eq!(cold.project, after);
}
#[test]
fn guarded_settings_checkpoint_five_refuses_downgraded_forged_rehashed_or_cross_plane_receipts_atomically() {
    let dir = Scratch::new();
    let mut s = setup(true);
    s.path = Some(dir.0.join("generated.fcproj").to_string_lossy().into_owned());
    select(&mut s, 161200);
    let setting = mode(&s, "Touch");
    let p = request(&mut s, 161201, setting);
    s.execute(COMMAND, p).unwrap();
    let saved = save(&mut s, 161202);
    let original = std::fs::read(dir.companion()).unwrap();
    for kind in ["version1", "version2", "version3", "version4", "authority", "scope", "project", "sequence", "redo", "proof", "alias"] {
        let mut v: Value = serde_json::from_slice(&original).unwrap();
        if kind.starts_with("version") {
            v["version"] = json!(kind[7..].parse::<u64>().unwrap());
        } else {
            let t = &mut v["tickets"][1];
            match kind {
                "authority" => t["result"]["writeAuthority"] = json!(true),
                "scope" => t["project_scope"]["workspaceId"] = json!(C),
                "project" => t["after"] = json!(999999),
                "sequence" => t["after_sequence"] = Value::Null,
                "redo" => t["redo_consumed"] = json!(true),
                "proof" => {
                    let mut inner: Value = serde_json::from_str(t["result"]["editorProof"]["data"].as_str().unwrap()).unwrap();
                    inner["afterEditor"]["snapping"] = json!(false);
                    t["result"]["editorProof"]["data"] = json!(serde_json::to_string(&inner).unwrap());
                    rehash(&mut t["result"]["editorProof"]);
                }
                _ => t["request"]["setting"] = default("Constant Gain"),
            }
        }
        let bytes = serde_json::to_vec(&v).unwrap();
        std::fs::write(dir.companion(), &bytes).unwrap();
        let mut forged = saved.clone();
        forged["checkpoint"]["sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        let mut cold = reopened(&s);
        let before = state(&cold);
        assert!(restore(&mut cold, 161203, &forged).is_err(), "accepted {kind}");
        assert_eq!(state(&cold), before);
    }
    std::fs::write(dir.companion(), original).unwrap();
}
