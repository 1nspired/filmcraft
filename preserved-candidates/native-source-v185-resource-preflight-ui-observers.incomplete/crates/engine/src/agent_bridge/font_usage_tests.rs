use super::{project_plane, tests::token};
use crate::Session;
use filmcraft_text::fonts::{self, FaceData, FaceInfo, FontSource};
use serde_json::{Value, json};
fn scope() -> Value {
    json!({"projectId":"p_font_usage","workspaceId":"6ae17621-32f1-4e2b-8940-aef47d1279a5"})
}
fn setup() -> Session {
    let mut s = Session::default();
    s.execute("file.newProject", json!({"name":"Font usage","workspace":scope()})).unwrap();
    s.path = Some("/generated/font-usage.fcproj".into());
    s
}
fn read(s: &mut Session, n: usize) {
    let p = s.execute(project_plane::READ, json!({"token":token(n),"scope":scope(),"projectPath":s.path})).unwrap();
    assert_eq!(p["capabilities"]["fontUsage"], true);
}
fn args(n: usize, offset: usize) -> Value {
    json!({"token":token(n),"scope":scope(),"offset":offset,"limit":16})
}
fn state(s: &Session) -> Value {
    json!({"project":s.project,"editor":s.state,"revision":s.revision,"undo":s.history.undo,"redo":s.history.redo,"journal":s.journal,"path":s.path})
}
#[test]
fn authored_font_usage_reads_empty_owning_project_without_inventing_a_sequence_or_authority() {
    let mut s = setup();
    read(&mut s, 85000);
    let before = state(&s);
    let p = s.execute("agent.readFontsUsed", args(85000, 0)).unwrap();
    assert_eq!(p["total"], 0);
    assert!(p["sequence"].is_null());
    assert_eq!(p["resourceEvidence"], false);
    assert_eq!(p["writeAuthority"], false);
    assert_eq!(p["keyframeInventory"], false);
    assert_eq!(p["sourceValidation"], "not-performed");
    assert_eq!(state(&s), before);
}
#[test]
fn authored_font_usage_matches_complete_original_human_rows_including_shared_masters_runs_and_captions() {
    let mut s = setup();
    s.execute("file.newSequence", json!({"name":"Owned","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
    let c = s.execute("graphics.newText", json!({"text":"Shared","font":"Noto Serif","fontStyle":"Regular"})).unwrap()["clip"].as_u64().unwrap();
    s.execute("graphics.setCharStyle", json!({"clip":c,"layer":0,"start":0,"end":2,"style":{"font":"Inter"}})).unwrap();
    s.execute("graphics.upgradeToSourceGraphic", json!({"clip":c})).unwrap();
    s.execute("captions.add", json!({"text":"Caption","seconds":1,"durationSeconds":1})).unwrap();
    read(&mut s, 85010);
    let expected = s.execute("graphics.fonts.used", json!({})).unwrap();
    let before = state(&s);
    let mut offset = 0;
    let mut all = Vec::new();
    let mut sha = None;
    loop {
        let p = s.execute("agent.readFontsUsed", args(85010, offset)).unwrap();
        if let Some(h) = &sha {
            assert_eq!(&p["usageSha256"], h);
        } else {
            sha = Some(p["usageSha256"].clone());
        }
        for row in p["rows"].as_array().unwrap() {
            let mut row = row.clone();
            row.as_object_mut().unwrap().remove("position");
            all.push(row);
        }
        if let Some(n) = p["nextOffset"].as_u64() {
            assert!(n > offset as u64);
            offset = n as usize;
        } else {
            break;
        }
    }
    assert_eq!(all, expected.as_array().unwrap().clone());
    assert!(all.iter().any(|r| r["style"] == "SemiBold"));
    assert_eq!(state(&s), before);
}
struct Face {
    family: String,
}
impl FontSource for Face {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::Static(fonts::INTER_REGULAR),
            origin: "generated-test",
        }]
    }
}
#[test]
fn authored_font_usage_refreshes_resolution_hash_and_refuses_foreign_stale_or_hostile_requests_without_writes() {
    let mut s = setup();
    s.execute("file.newSequence", json!({"name":"Owned","fps":24,"width":128,"height":128,"video":1,"audio":1})).unwrap();
    let family = format!("Poema Usage Late {}", std::process::id());
    s.execute("graphics.newText", json!({"text":"Late","font":family})).unwrap();
    read(&mut s, 85020);
    let a = s.execute("agent.readFontsUsed", args(85020, 0)).unwrap();
    assert!(a["rows"].as_array().unwrap().iter().any(|r| r["family"] == family && r["missing"] == true));
    assert_eq!(fonts::add_source(&Face { family: family.clone() }), 1);
    let b = s.execute("agent.readFontsUsed", args(85020, 0)).unwrap();
    assert_ne!(a["usageSha256"], b["usageSha256"]);
    assert_eq!(a["projectSha256"], b["projectSha256"]);
    let before = state(&s);
    for i in 0..5 {
        let mut p = args(85020, 0);
        match i {
            0 => p["scope"]["projectId"] = json!("foreign"),
            1 => p["offset"] = json!(100001),
            2 => p["limit"] = json!(0),
            3 => p["projection"] = json!(true),
            _ => p["token"] = json!(token(999999)),
        };
        assert!(s.execute("agent.readFontsUsed", p).is_err());
    }
    assert_eq!(state(&s), before);
    s.execute("file.newBin", json!({"name":"Later"})).unwrap();
    let later = state(&s);
    assert!(s.execute("agent.readFontsUsed", args(85020, 0)).is_err());
    assert_eq!(state(&s), later);
}
