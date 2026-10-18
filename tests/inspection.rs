use serde_json::{Value, json};
use waifu_studio::{VrmVersion, inspect_bytes};

fn document(v1: bool) -> Value {
    let mut doc =
        json!({"asset":{"version":"2.0"}, "nodes":[{}], "scenes":[{"nodes":[0]}], "meshes":[{}]});
    doc["extensions"] = if v1 {
        json!({"VRMC_vrm":{"specVersion":"1.0","meta":{"name":"Test avatar","authors":["Example"],"licenseUrl":"https://vrm.dev/licenses/1.0/"},"humanoid":{"humanBones":{"hips":{"node":0}}}}})
    } else {
        json!({"VRM":{"specVersion":"0.0","meta":{"title":"Test avatar","author":"Example","licenseName":"CC0"},"humanoid":{"humanBones":[{"bone":"hips","node":0}]}}})
    };
    doc
}

fn glb(doc: &Value) -> Vec<u8> {
    let mut json = serde_json::to_vec(doc).unwrap();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bytes = b"glTF".to_vec();
    bytes.extend(2_u32.to_le_bytes());
    bytes.extend(((20 + json.len()) as u32).to_le_bytes());
    bytes.extend((json.len() as u32).to_le_bytes());
    bytes.extend(b"JSON");
    bytes.extend(json);
    bytes
}

#[test]
fn both_vrm_generations_preserve_identity_and_license_metadata() {
    for v1 in [false, true] {
        let result = inspect_bytes(&glb(&document(v1))).unwrap();
        assert_eq!(
            result.version,
            if v1 { VrmVersion::V1 } else { VrmVersion::V0 }
        );
        assert_eq!(result.name, "Test avatar");
        assert_eq!(result.authors, ["Example"]);
        assert_eq!(result.humanoid_bones["hips"], 0);
        assert!(
            result
                .meta
                .get(if v1 { "licenseUrl" } else { "licenseName" })
                .is_some()
        );
    }
}

#[test]
fn truncated_and_length_mismatched_containers_are_errors_not_panics() {
    let bytes = glb(&document(true));
    for length in 0..bytes.len() {
        assert!(inspect_bytes(&bytes[..length]).is_err());
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(inspect_bytes(&trailing).is_err());
}

#[test]
fn ordinary_glb_is_not_misidentified_as_vrm() {
    let mut doc = document(true);
    doc.as_object_mut().unwrap().remove("extensions");
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("missing VRM")
    );
}

#[test]
fn rejects_bone_indices_outside_the_node_array() {
    let mut doc = document(true);
    doc["extensions"]["VRMC_vrm"]["humanoid"]["humanBones"]["hips"]["node"] = json!(9);
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("missing node")
    );
}

#[test]
fn rejects_unsupported_versions_and_ambiguous_extensions() {
    let mut doc = document(true);
    doc["extensions"]["VRMC_vrm"]["specVersion"] = json!("2.0");
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("specVersion")
    );
    doc["extensions"]["VRM"] = document(false)["extensions"]["VRM"].clone();
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
}

#[test]
fn rejects_duplicate_bones_and_models_without_scenes() {
    let mut doc = document(false);
    doc["extensions"]["VRM"]["humanoid"]["humanBones"] =
        json!([{"bone":"hips","node":0},{"bone":"hips","node":0}]);
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("duplicate")
    );
    doc["scenes"] = json!([]);
    assert!(
        inspect_bytes(&glb(&doc))
            .unwrap_err()
            .to_string()
            .contains("no scene")
    );
}
