//! Native resource observations must hash the renderer's retained bytes,
//! without loading files or copying them merely to inspect a font.
use crate::fonts::{self, FaceData, FaceInfo, FontSource};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct FileFace {
    family: String,
    file: PathBuf,
}
impl FontSource for FileFace {
    fn faces(&self) -> Vec<FaceInfo> {
        vec![FaceInfo {
            family: self.family.clone(),
            style: "Regular".into(),
            weight: 400,
            italic: false,
            index: 0,
            data: FaceData::File(self.file.clone()),
            origin: "generated-test",
        }]
    }
}
fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("Poema Font Resource {} {}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

#[test]
fn loaded_font_bytes_do_not_trigger_load_and_survive_disk_replacement() {
    let family = unique();
    let root = std::env::temp_dir().join(format!("poema-font-bytes-{}", family.replace(' ', "-")));
    std::fs::create_dir(&root).unwrap();
    let file = root.join("generated-font.ttf");
    std::fs::write(&file, fonts::INTER_REGULAR).unwrap();
    assert_eq!(fonts::add_source(&FileFace { family: family.clone(), file: file.clone() }), 1);
    let face = fonts::all_faces().into_iter().find(|f| f.info.family == family).unwrap();
    // A getter that accidentally invokes the original loader here would cache
    // failure forever. Recreating the file must still permit its first load.
    std::fs::remove_file(&file).unwrap();
    assert!(face.loaded_data().is_none());
    std::fs::write(&file, fonts::INTER_REGULAR).unwrap();
    assert!(face.font().is_some());
    let borrowed = face.loaded_data().unwrap();
    assert_eq!(borrowed, fonts::INTER_REGULAR);
    assert_eq!(borrowed.as_ptr(), face.loaded_data().unwrap().as_ptr());
    std::fs::write(&file, fonts::INTER_ITALIC).unwrap();
    assert_eq!(face.loaded_data().unwrap(), fonts::INTER_REGULAR);
    assert_eq!(face.data().unwrap(), fonts::INTER_REGULAR);
    // A distinct new native face uses the new disk generation through the
    // existing loader; an old face's retained buffer must stay unchanged.
    let second_family = unique();
    assert_eq!(fonts::add_source(&FileFace { family: second_family.clone(), file: file.clone() }), 1);
    let second = fonts::all_faces().into_iter().find(|f| f.info.family == second_family).unwrap();
    assert!(second.loaded_data().is_none());
    assert!(second.font().is_some());
    assert_eq!(second.loaded_data().unwrap(), fonts::INTER_ITALIC);
    std::fs::remove_file(&file).unwrap();
    assert_eq!(face.loaded_data().unwrap(), fonts::INTER_REGULAR);
    assert_eq!(second.loaded_data().unwrap(), fonts::INTER_ITALIC);
}

#[test]
fn loaded_shared_font_bytes_borrow_the_original_registered_buffer() {
    let ids = fonts::add_font_data(fonts::INTER_REGULAR.to_vec());
    let face = fonts::face(*ids.first().unwrap());
    assert!(face.loaded_data().is_none());
    assert!(face.font().is_some());
    let FaceData::Shared(original) = &face.info.data else { panic!("registered memory face must retain its shared buffer") };
    assert_eq!(face.loaded_data().unwrap(), fonts::INTER_REGULAR);
    assert_eq!(face.loaded_data().unwrap().as_ptr(), original.as_ptr());
}

#[test]
fn loaded_font_bytes_do_not_imply_parse_success_or_retry_failed_loads() {
    let family = unique();
    let root = std::env::temp_dir().join(format!("poema-invalid-font-bytes-{}", family.replace(' ', "-")));
    std::fs::create_dir(&root).unwrap();
    let file = root.join("generated-invalid.ttf");
    let invalid = b"generated invalid font bytes";
    std::fs::write(&file, invalid).unwrap();
    assert_eq!(fonts::add_source(&FileFace { family: family.clone(), file: file.clone() }), 1);
    let face = fonts::all_faces().into_iter().find(|f| f.info.family == family).unwrap();
    assert!(face.loaded_data().is_none());
    assert!(face.font().is_none());
    assert_eq!(face.loaded_data().unwrap(), invalid);
    std::fs::write(&file, fonts::INTER_REGULAR).unwrap();
    assert!(face.font().is_none());
    assert_eq!(face.loaded_data().unwrap(), invalid);

    let failed_family = unique();
    assert_eq!(fonts::add_source(&FileFace { family: failed_family.clone(), file: file.clone() }), 1);
    let failed = fonts::all_faces().into_iter().find(|f| f.info.family == failed_family).unwrap();
    std::fs::remove_file(&file).unwrap();
    assert!(failed.font().is_none());
    assert!(failed.loaded_data().is_none());
    std::fs::write(&file, fonts::INTER_REGULAR).unwrap();
    // Observing a failed load must not retry it or switch renderer generation.
    assert!(failed.loaded_data().is_none());
    assert!(failed.font().is_none());
    assert!(failed.loaded_data().is_none());
    std::fs::remove_file(&file).unwrap();
}
