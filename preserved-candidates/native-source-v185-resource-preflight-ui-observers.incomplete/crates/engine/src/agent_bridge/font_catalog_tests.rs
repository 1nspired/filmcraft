use super::{project_plane, tests::token};
use crate::Session;
use serde_json::{Value, json};
fn scope() -> Value {
    json!({"projectId":"p_font_catalog","workspaceId":"6ae17621-32f1-4e2b-8940-aef47d1279a5"})
}
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Font catalog","workspace":scope()})).unwrap();
    s.path = Some("/generated/font-catalog.fcproj".into());
    s
}
fn read(s: &mut Session, n: usize) {
    s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"state":s.state,"revision":s.revision,"saved":s.saved_revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"path":s.path})
}
fn request(n: usize, offset: usize) -> Value {
    json!({"token":token(n),"realm":{"kind":"project","scope":scope()},"system":false,"offset":offset,"limit":16})
}
#[test]
fn font_catalog_uses_original_human_families_without_inventing_a_sequence_or_project_edits() {
    let mut s = setup();
    read(&mut s, 81100);
    let before = state(&s);
    let expected = s.execute("fonts.list", json!({"system":false})).unwrap();
    let expected = expected
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["styles"].as_array().unwrap().iter().map(move |style| (f["family"].clone(), style.clone())))
        .collect::<Vec<_>>();
    let mut offset = 0;
    let mut rows = Vec::new();
    let mut digest = None;
    loop {
        let p = s.execute("agent.readFontCatalog", request(81100, offset)).unwrap();
        assert!(p["sequence"].is_null());
        assert_eq!(p["resourceEvidence"], false);
        assert_eq!(p["writeAuthority"], false);
        assert_eq!(p["syntheticStyleOptions"], json!(["Bold", "Italic", "Bold Italic"]));
        if let Some(d) = &digest {
            assert_eq!(&p["catalogSha256"], d);
        } else {
            digest = Some(p["catalogSha256"].clone());
        }
        rows.extend(p["rows"].as_array().unwrap().iter().map(|r| (r["family"].clone(), r["style"].clone())));
        match p["nextOffset"].as_u64() {
            Some(n) => {
                assert!(n > offset as u64);
                offset = n as usize;
            }
            None => break,
        }
    }
    assert_eq!(rows, expected);
    assert_eq!(state(&s), before);
    assert!(s.project.sequences().next().is_none());
}
#[test]
fn font_catalog_refuses_stale_foreign_wrong_realm_and_hostile_requests_without_changes() {
    let mut s = setup();
    read(&mut s, 81110);
    let good = request(81110, 0);
    let before = state(&s);
    for mut bad in [good.clone(), good.clone(), good.clone(), good.clone(), good.clone(), good.clone(), good.clone()].into_iter().enumerate() {
        match bad.0 {
            0 => bad.1["realm"]["scope"]["projectId"] = json!("foreign"),
            1 => bad.1["realm"]["kind"] = json!("sequence"),
            2 => bad.1["system"] = json!(1),
            3 => bad.1["limit"] = json!(0),
            4 => bad.1["offset"] = json!(100001),
            5 => bad.1["projection"] = json!(true),
            _ => bad.1["token"] = json!(token(999999)),
        }
        assert!(s.execute("agent.readFontCatalog", bad.1).is_err());
    }
    assert_eq!(state(&s), before);
    s.execute("file.newBin", json!({"name":"Later human work"})).unwrap();
    let later = state(&s);
    assert!(s.execute("agent.readFontCatalog", good).is_err());
    assert_eq!(state(&s), later);
}
#[test]
fn font_catalog_accepts_original_owning_sequence_tickets_but_never_elevates_catalog_to_write_authority() {
    let mut s = Session::default();
    s.execute("file.newSequence", json!({"name":"Sequence","fps":24,"width":128,"height":128,"video":1,"audio":0})).unwrap();
    let scope = json!({"projectId":"p_catalog_sequence","anchorSequence":s.state.active_sequence.unwrap().0});
    s.execute("project.associateEditorialOwner", json!({"scope":scope,"workspaceId":"6ae17621-32f1-4e2b-8940-aef47d1279a5"})).unwrap();
    s.execute("poema.trackSequence", json!({})).unwrap();
    s.execute("poema.trackEffects", json!({})).unwrap();
    super::tests::read(&mut s, 81120);
    let before = state(&s);
    let request = json!({"token":token(81120),"realm":{"kind":"sequence","scope":scope},"system":false,"offset":0,"limit":16});
    let p = s.execute("agent.readFontCatalog", request.clone()).unwrap();
    assert_eq!(p["writeAuthority"], false);
    assert_eq!(p["sequence"], s.state.active_sequence.unwrap().0);
    let mut wrong = request;
    wrong["realm"]["scope"]["projectId"] = json!("foreign");
    assert!(s.execute("agent.readFontCatalog", wrong).is_err());
    assert!(s.execute("agent.apply",json!({"token":token(81120),"label":"Catalog cannot edit","exact":true,"authority":"document_state","operations":[{"command":"agent.readFontCatalog","params":p}]})).is_err());
    assert_eq!(state(&s), before);
}
